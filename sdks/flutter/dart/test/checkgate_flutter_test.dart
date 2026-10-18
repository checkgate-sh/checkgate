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
}
