import 'dart:async';
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

import '../checkgate_flutter.dart';

/// Records every request a MockClient receives. The SDK reports impressions and
/// events fire-and-forget, and MockClient invokes its handler asynchronously —
/// so tests `await pumpEventQueue()` after a flush before asserting.
class Recorder {
  final List<http.Request> requests = [];
  MockClient client() => MockClient((req) async {
        requests.add(req);
        return http.Response('', 204);
      });
  http.Request get last => requests.last;
}

String? auth(http.Request r) =>
    r.headers['authorization'] ?? r.headers['Authorization'];

void main() {
  late Recorder rec;
  CheckgateClient client({int batch = 50}) => CheckgateClient(
        serverUrl: 'http://localhost:9999',
        sdkKey: 'sk_test',
        impressionBatchSize: batch,
        httpClient: rec.client(),
      );

  setUp(() => rec = Recorder());

  test('constructs without loading the native library or a connection', () {
    // Would throw if the constructor still eagerly opened the FFI library.
    final c = client();
    expect(c.isReady, isFalse);
    expect(c.debugEvents, isEmpty);
    expect(c.debugImpressions, isEmpty);
  });

  group('goal-event tracking', () {
    test('buffers and flushes to /events with Bearer auth', () async {
      final c = client(batch: 2)..debugSetEnvironment('env-123');
      c.track('checkout_complete', 'u1', value: 49.99);
      await pumpEventQueue();
      expect(rec.requests, isEmpty, reason: 'below batch size');

      c.track('checkout_complete', 'u2');
      await pumpEventQueue();

      expect(rec.requests, hasLength(1));
      expect(rec.last.method, 'POST');
      expect(rec.last.url.path, '/api/environments/env-123/events');
      expect(auth(rec.last), 'Bearer sk_test');
      final body = jsonDecode(rec.last.body) as List;
      expect(body, hasLength(2));
      expect(body[0]['event_key'], 'checkout_complete');
      expect(body[0]['user_id'], 'u1');
      expect(body[0]['value'], 49.99);
    });

    test('drops invalid keys and pre-connect calls (no environment)', () async {
      final c = client(); // no debugSetEnvironment → _envId null
      c.track('', 'u');
      c.track('goal', 'u');
      await pumpEventQueue();
      expect(rec.requests, isEmpty);
      expect(c.debugEvents, isEmpty);
    });
  });

  group('impressions', () {
    test('flush posts to /impressions and caps at 500 per batch', () async {
      final c = client()..debugSetEnvironment('env-123');
      for (var i = 0; i < 600; i++) {
        c.debugImpressions.add({'flag_key': 'f', 'user_id': 'u$i', 'value': 'true'});
      }
      c.debugFlush();
      // The buffer is trimmed synchronously; the POST is dispatched async.
      expect(c.debugImpressions, hasLength(100));
      await pumpEventQueue();
      expect(rec.requests, hasLength(1));
      expect(rec.last.url.path, '/api/environments/env-123/impressions');
      expect((jsonDecode(rec.last.body) as List), hasLength(500));
    });

    test('nothing is sent when there is no environment yet', () async {
      final c = client();
      c.debugImpressions.add({'flag_key': 'f', 'user_id': 'u', 'value': 'true'});
      c.debugFlush();
      await pumpEventQueue();
      expect(rec.requests, isEmpty);
    });
  });

  test('value serialization matches the other SDKs', () {
    final c = client();
    expect(c.debugFormatValue(null), 'null');
    expect(c.debugFormatValue(true), 'true');
    expect(c.debugFormatValue(42), '42');
    expect(c.debugFormatValue('blue'), 'blue');
    expect(c.debugFormatValue({'a': 1}), '{"a":1}');
  });

  test('close() flushes buffered impressions and events', () async {
    final c = client()..debugSetEnvironment('env-123');
    c.debugImpressions.add({'flag_key': 'f', 'user_id': 'u', 'value': 'true'});
    c.track('goal', 'u'); // buffered (below default batch of 50)
    c.close();
    await pumpEventQueue();
    final paths = rec.requests.map((r) => r.url.path).toList();
    expect(paths.any((p) => p.endsWith('/impressions')), isTrue);
    expect(paths.any((p) => p.endsWith('/events')), isTrue);
  });

  group('stream resync and ordering', () {
    void send(CheckgateClient c, String event, Object data) =>
        c.debugHandleSseEvent(event, jsonEncode(data));
    void upsert(CheckgateClient c, String key, bool enabled, [int? revision]) =>
        send(c, 'update', {
          'type': 'UPSERT',
          'flag': {'key': key, 'is_enabled': enabled},
          if (revision != null) 'revision': revision,
        });
    bool? enabled(CheckgateClient c, String key) =>
        c.debugFlags[key]?['is_enabled'] as bool?;

    test('a poll response that lands after the stream took over is dropped',
        () async {
      final release = Completer<void>();
      var requests = 0;
      final c = CheckgateClient(
        serverUrl: 'http://localhost:9999',
        sdkKey: 'sk_test',
        httpClient: MockClient((req) async {
          requests++;
          await release.future; // a slow server
          return http.Response(
              jsonEncode([
                {'key': 'checkout', 'is_enabled': false}
              ]),
              200);
        }),
      );
      final first = c.debugPollSnapshot();
      final second = c.debugPollSnapshot();
      await pumpEventQueue();
      expect(requests, 1, reason: 'ticks share the request in flight');

      // The stream reconnects and delivers a newer state meanwhile.
      send(c, 'connected', {'environment_id': 'env-1'});
      upsert(c, 'checkout', true, 7);
      send(c, 'ready', 1);
      expect(enabled(c, 'checkout'), isTrue);

      release.complete();
      await Future.wait([first, second]);
      expect(enabled(c, 'checkout'), isTrue,
          reason: 'the stale poll must not roll the flag back');
      upsert(c, 'checkout', false, 6);
      expect(enabled(c, 'checkout'), isTrue,
          reason: 'revisions survive, so an older event is still ignored');
    });

    test('a reconnect keeps serving the last snapshot until "ready"', () {
      final c = client();
      send(c, 'connected', {'environment_id': 'env-1'});
      upsert(c, 'checkout', true, 1);
      upsert(c, 'search', true, 2);
      send(c, 'ready', 2);
      expect(c.isReady, isTrue);

      send(c, 'connected', {'environment_id': 'env-1'});
      upsert(c, 'checkout', false, 3);
      expect(c.isReady, isTrue);
      expect(enabled(c, 'checkout'), isTrue, reason: 'old snapshot mid-replay');
      expect(enabled(c, 'search'), isTrue);

      final changed = <String>[];
      c.onChange(changed.add);
      send(c, 'ready', 1);
      expect(enabled(c, 'checkout'), isFalse);
      expect(c.debugFlags.containsKey('search'), isFalse);
      expect(changed..sort(), ['checkout', 'search']);
    });

    test('a replay cut short by a dropped stream is discarded', () {
      final c = client();
      send(c, 'connected', {'environment_id': 'env-1'});
      upsert(c, 'checkout', true, 1);
      send(c, 'ready', 1);

      send(c, 'connected', {'environment_id': 'env-1'});
      upsert(c, 'checkout', false, 2);
      c.debugStreamDropped();
      send(c, 'ready', 1); // a stray "ready" without a replay changes nothing
      expect(enabled(c, 'checkout'), isTrue);
    });

    test('reconnect listeners run after the whole snapshot is applied', () {
      final c = client();
      void boot(bool on) {
        send(c, 'connected', {'environment_id': 'env-1'});
        upsert(c, 'a-dependent', on, on ? 3 : 1);
        upsert(c, 'z-base', on, on ? 4 : 2);
        send(c, 'ready', 2);
      }

      boot(false);
      final seen = <String, bool?>{};
      c.onChange((key) => seen[key] = enabled(c, 'z-base'));
      boot(true);
      expect(seen, {'a-dependent': true, 'z-base': true});
    });

    test('a bootstrap tombstone keeps a delayed older event out', () {
      final c = client();
      send(c, 'connected', {'environment_id': 'env-1'});
      send(c, 'update', {'type': 'DELETE', 'key': 'ghost', 'revision': 7});
      send(c, 'ready', 0);
      upsert(c, 'ghost', true, 6);
      expect(c.debugFlags.containsKey('ghost'), isFalse);
      upsert(c, 'ghost', true, 8);
      expect(enabled(c, 'ghost'), isTrue);
    });

    test('change events older than the applied revision are ignored', () {
      final c = client();
      send(c, 'connected', {'environment_id': 'env-1'});
      upsert(c, 'checkout', false, 10);
      send(c, 'ready', 1);

      upsert(c, 'checkout', true, 12);
      upsert(c, 'checkout', false, 11);
      expect(enabled(c, 'checkout'), isTrue);

      send(c, 'update', {'type': 'DELETE', 'key': 'checkout', 'revision': 12});
      upsert(c, 'checkout', true, 12);
      expect(c.debugFlags.containsKey('checkout'), isFalse);
      upsert(c, 'checkout', true, 13);
      expect(enabled(c, 'checkout'), isTrue);
      send(c, 'update', {'type': 'DELETE', 'key': 'checkout', 'revision': 12});
      expect(enabled(c, 'checkout'), isTrue);

      upsert(c, 'legacy', true);
      upsert(c, 'legacy', false);
      expect(enabled(c, 'legacy'), isFalse);
    });
  });

  test('conversions carry the time they happened', () {
    final c = client()..debugSetEnvironment('env-123');
    final before = DateTime.now().toUtc();
    c.track('purchase', 'u1');
    final at = DateTime.parse(c.debugEvents.single['occurred_at'] as String);
    expect(at.isBefore(before), isFalse);
    expect(at.isAfter(DateTime.now().toUtc()), isFalse);
  });

  group('SSE framing', () {
    // One frame per event the server sends during a bootstrap, plus a
    // keep-alive comment, a multi-byte character and a trailing `ready`.
    const frames = 'event: connected\ndata: {"environment_id":"env-1"}\n\n'
        ': keep-alive-text\n\n'
        'event: update\ndata: {"type":"UPSERT","flag":{"key":"caf\u00e9-\u{1F680}"}}\n\n'
        'event: ready\ndata: 1\n\n';
    const expected = [
      MapEntry('connected', '{"environment_id":"env-1"}'),
      MapEntry('update', '{"type":"UPSERT","flag":{"key":"caf\u00e9-\u{1F680}"}}'),
      MapEntry('ready', '1'),
    ];

    Future<List<MapEntry<String, String>>> parse(List<List<int>> chunks) =>
        parseSseEvents(Stream.fromIterable(chunks)).toList();

    String show(List<MapEntry<String, String>> events) =>
        events.map((e) => '${e.key}=${e.value}').join(' | ');

    test('parses whole frames', () async {
      expect(show(await parse([utf8.encode(frames)])), show(expected));
    });

    test('reassembles lines split at every byte offset', () async {
      // Socket reads end anywhere: inside `event:`, inside a JSON payload,
      // inside a UTF-8 sequence, or between the two newlines of a frame.
      final bytes = utf8.encode(frames);
      for (var cut = 1; cut < bytes.length; cut++) {
        final events =
            await parse([bytes.sublist(0, cut), bytes.sublist(cut)]);
        expect(show(events), show(expected), reason: 'cut at byte $cut');
      }
    });

    test('reassembles a stream delivered one byte at a time', () async {
      final bytes = utf8.encode(frames);
      final events = await parse([for (final b in bytes) [b]]);
      expect(show(events), show(expected));
    });
  });
}
