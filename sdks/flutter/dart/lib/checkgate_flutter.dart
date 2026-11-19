/// Checkgate Flutter SDK
///
/// Usage:
/// ```dart
/// final client = CheckgateClient(
///   serverUrl: 'https://flags.example.com',
///   sdkKey: 'my-sdk-key',
/// );
/// await client.connect();
///
/// final enabled = client.isEnabled('dark_mode', userId, {'country': 'US'});
/// ```
library checkgate_flutter;

import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:math';

import 'package:ffi/ffi.dart';
import 'package:http/http.dart' as http;

import 'checkgate_bindings.dart';

/// Splits a raw SSE byte stream into `(event name, data)` pairs.
///
/// Network reads end wherever the socket does — mid-line or even inside a
/// multi-byte UTF-8 character — so the bytes are decoded and re-split into
/// whole lines (`LineSplitter` carries a partial line over to the next chunk)
/// before any line is interpreted. Splitting each chunk on `\n` instead lost
/// the second half of every line that straddled two reads, which dropped
/// flags from large bootstraps and could lose the `ready` event entirely.
///
/// [onFrame] runs at the end of every frame, including frames that carry no
/// event (an SSE comment such as an older server's keep-alive): the client's
/// proof that the stream is alive.
Stream<MapEntry<String, String>> parseSseEvents(Stream<List<int>> bytes,
    {void Function()? onFrame}) async* {
  var eventName = '';
  final data = StringBuffer();
  await for (final line
      in bytes.transform(utf8.decoder).transform(const LineSplitter())) {
    if (line.startsWith('event:')) {
      eventName = line.substring(6).trim();
    } else if (line.startsWith('data:')) {
      data.write(line.substring(5).trim());
    } else if (line.isEmpty) {
      onFrame?.call();
      if (data.isNotEmpty) {
        yield MapEntry(eventName, data.toString());
        data.clear();
      }
      eventName = '';
    }
  }
}

/// HTTP statuses that mean the SDK key itself is rejected: retrying cannot help.
const Set<int> _authFailureStatuses = {401, 403};

/// A healthy stream is never silent for longer than the server's heartbeat
/// interval; it is declared dead after this many intervals without a byte.
const int _idleHeartbeats = 3;

/// The keep-alive interval of servers that do not announce one (before
/// 0.1.31). They send it as an SSE comment, which this client sees as bytes.
const Duration _legacyKeepAlive = Duration(seconds: 15);

/// The server rejected the SDK key with HTTP 401 or 403. The client stops
/// retrying (a rejected key does not become valid by asking again) and keeps
/// serving the flags it already has. Call [CheckgateClient.connect] again to
/// retry, e.g. after the key was restored.
class CheckgateAuthException implements Exception {
  CheckgateAuthException(this.statusCode);

  /// The HTTP status the server answered with.
  final int statusCode;

  @override
  String toString() => 'CheckgateAuthException: the server rejected the SDK '
      'key (HTTP $statusCode). Not retrying; check the key, then call '
      'connect() again.';
}

/// Where the client stands with the server; see [CheckgateClient.status].
enum CheckgateConnectionState {
  /// [CheckgateClient.connect] has not been called.
  idle,

  /// First connection attempt in progress.
  connecting,

  /// Live stream; flags are current.
  streaming,

  /// The stream was lost; retrying with backoff.
  reconnecting,

  /// The stream keeps failing; polling `GET /flags/snapshot` meanwhile.
  polling,

  /// The SDK key was rejected; not retrying.
  authFailed,

  /// [CheckgateClient.close] was called.
  closed,
}

/// Connection health, for detecting stale flags.
class CheckgateStatus {
  const CheckgateStatus(this.state, this.lastSyncAt, this.error);

  final CheckgateConnectionState state;

  /// When the client last confirmed its flags are current: a completed
  /// bootstrap or poll, or any traffic on a live stream (heartbeats
  /// included). `null` before the first. Flags are served from the last
  /// known state in every state; their age is `DateTime.now() - lastSyncAt`.
  final DateTime? lastSyncAt;

  /// The error behind [CheckgateConnectionState.authFailed], otherwise null.
  final Object? error;
}

/// Optional persistence adapter. Implement with your storage of choice
/// (e.g. `shared_preferences`) so the last flag snapshot survives restarts and
/// flags can be evaluated before (or without) a live connection.
abstract class CheckgateStorage {
  Future<String?> read(String key);
  Future<void> write(String key, String value);
}

class CheckgateClient {
  final String serverUrl;
  final String sdkKey;

  /// Report evaluation events to the server for analytics. Defaults to true.
  final bool reportImpressions;

  /// Flush buffered impressions once this many have accumulated.
  final int impressionBatchSize;

  /// Periodic impression flush interval.
  final Duration impressionFlushInterval;

  /// Include user attributes as `context` in reported impressions. Off by
  /// default so attributes never leave the device.
  final bool sendEvaluationContext;

  /// Base delay before the first reconnect attempt; doubles each subsequent
  /// attempt (with jitter) up to [maxReconnectDelay].
  final Duration reconnectBaseDelay;

  /// Upper bound on the reconnect backoff delay.
  final Duration maxReconnectDelay;

  /// Consecutive failed SSE reconnects before falling back to polling
  /// `GET /flags/snapshot` (e.g. when a proxy blocks long-lived connections).
  final int pollFallbackThreshold;

  /// Poll interval while in fallback mode.
  final Duration pollInterval;

  /// Optional persistence adapter for offline flag evaluation.
  final CheckgateStorage? storage;

  /// A stream attempt that has not received the server's `connected` event
  /// within this time is abandoned and retried.
  final Duration connectTimeout;

  /// A connected stream that receives nothing (heartbeats included) for this
  /// long is treated as dead and reconnected. Default (null): 3 × the
  /// heartbeat interval the server announces, 45 s by default. [Duration.zero]
  /// disables it.
  final Duration? streamIdleTimeout;

  // Connection health (see [status]).
  CheckgateConnectionState _state = CheckgateConnectionState.idle;
  DateTime? _lastSyncAt;
  Object? _lastError;

  /// Bumped by [connect] and [close]: an SSE loop from an earlier session
  /// sees the change and exits, so a quick close() + connect() never leaves
  /// two loops running.
  int _loopGeneration = 0;

  /// Ends the current stream attempt (its byte stream errors), for the
  /// watchdog and for [close].
  void Function()? _abortStream;
  Timer? _watchdog;
  DateTime? _deadline;
  Duration _idleTimeout = Duration.zero;

  // The native evaluation engine, loaded lazily on connect() so constructing a
  // client is cheap and side-effect-free (and unit-testable without the native
  // library). Non-null after connect(); evaluation methods guard on readiness.
  CheckgateBindings? _bindings;

  // Offline persistence: shadow copy of the raw flag set.
  final Map<String, Map<String, dynamic>> _flagCache = {};
  bool _hydrated = false;
  String get _cacheKey => 'checkgate:flags:$serverUrl';

  // Reconnect backoff state.
  int _reconnectAttempts = 0;
  final Random _rng = Random();

  // Poll fallback: used when SSE cannot be established at all. Reuses
  // _flagCache to diff each poll response against the last known state.
  bool _polling = false;
  Timer? _pollTimer;

  /// Bumped whenever polling stops (the stream took over, or [close]). A poll
  /// response that arrives after a bump is stale and is discarded.
  int _syncGeneration = 0;

  /// The poll request in flight, so ticks never stack up behind a slow server.
  Future<void>? _pollInFlight;

  /// Upper bound on one poll request.
  static const Duration _pollTimeout = Duration(seconds: 30);

  // Readiness: completed once the server has replayed the full flag set and
  // sent the "ready" event.
  bool _ready = false;
  bool _connecting = false;
  Completer<void>? _readyCompleter;

  // During a (re)connect replay: the flags received so far and their
  // revisions, swapped in as a whole on "ready" (see _handleSseEvent).
  Map<String, Map<String, dynamic>>? _staged;
  Map<String, _Revision>? _stagedRevisions;
  // Last applied revision per flag key, with the learn counter value when it
  // was applied. The counter grows with every applied change; a full set
  // compares it with the value at its request (see _mergeFullSet). Never a
  // clock.
  Map<String, _Revision> _revisions = {};
  int _learnSeq = 0;
  int _streamStartedAt = 0;
  final List<void Function(String)> _changeListeners = [];

  // SSE loop cancellation
  bool _closed = false;
  http.Client? _httpClient;

  // Impression reporting state
  String? _envId;
  final List<Map<String, dynamic>> _impressions = [];
  // Buffered goal/conversion events reported via track() (A/B testing).
  final List<Map<String, dynamic>> _events = [];
  Timer? _flushTimer;

  CheckgateClient({
    required this.serverUrl,
    required this.sdkKey,
    this.reportImpressions = true,
    this.impressionBatchSize = 50,
    this.impressionFlushInterval = const Duration(seconds: 10),
    this.sendEvaluationContext = false,
    this.reconnectBaseDelay = const Duration(seconds: 2),
    this.maxReconnectDelay = const Duration(seconds: 30),
    this.pollFallbackThreshold = 3,
    this.pollInterval = const Duration(seconds: 30),
    this.storage,
    this.connectTimeout = const Duration(seconds: 30),
    this.streamIdleTimeout,
    http.Client? httpClient,
    CheckgateBindings? bindings,
  }) {
    // Test seams: an injected client lets unit tests exercise the
    // impression/event reporting paths with a mock, without a real connection;
    // injected bindings load the engine from an explicit library (host tests).
    _httpClient = httpClient;
    _bindings = bindings;
  }

  /// Open the SSE stream. The returned future completes once the server has
  /// replayed the full flag set and sent the "ready" event — consistent with
  /// the Node and Web SDKs. The server sends the full flag state on connect,
  /// so no separate REST bootstrap call is needed.
  ///
  /// Completes with a [CheckgateAuthException] if the server rejects the SDK
  /// key (HTTP 401/403), and with a [StateError] if [close] is called first.
  /// While the server is unreachable it keeps retrying; add a timeout if the
  /// caller cannot wait. Flags already loaded keep being served either way.
  Future<void> connect() => _connect(loadEngine: true);

  Future<void> _connect({required bool loadEngine}) {
    if (_ready && _state != CheckgateConnectionState.authFailed) {
      return Future<void>.value();
    }
    if (_connecting) return _readyCompleter!.future;

    _connecting = true;
    _closed = false;
    _lastError = null;
    _state = _ready
        ? CheckgateConnectionState.reconnecting
        : CheckgateConnectionState.connecting;
    final generation = ++_loopGeneration;
    final completer = Completer<void>();
    _readyCompleter = completer;
    // A caller that never awaits must not get an uncaught async error; one
    // that awaits still receives it.
    completer.future.ignore();
    // Load the native engine lazily (once), and open an HTTP client unless one
    // was injected for testing.
    if (loadEngine) _bindings ??= CheckgateBindings.open();
    _httpClient ??= http.Client();
    // Hydrate from the persisted snapshot so evaluations work offline
    // immediately, then open the live stream.
    _hydrateFromCache().whenComplete(() {
      if (!_closed && generation == _loopGeneration) {
        _startSseLoop(generation, _httpClient!, Uri.parse('$serverUrl/stream'), {
          'Authorization': 'Bearer $sdkKey',
          'Accept': 'text/event-stream',
          'Cache-Control': 'no-cache',
        });
      }
    });
    return _readyCompleter!.future;
  }

  /// Whether the initial bootstrap has completed and flags are ready to evaluate.
  bool get isReady => _ready;

  /// Connection health, for detecting stale flags.
  CheckgateStatus get status => CheckgateStatus(_state, _lastSyncAt, _lastError);

  void _markReady() {
    _lastSyncAt = DateTime.now();
    _ready = true;
    final completer = _readyCompleter;
    if (completer != null && !completer.isCompleted) completer.complete();
  }

  /// Registers a listener invoked whenever a flag changes after the initial
  /// bootstrap (created, updated, or deleted). The flag's key is passed to the
  /// callback. The initial bootstrap does not trigger listeners; a reconnect
  /// re-sync triggers them only for flags that changed while disconnected.
  ///
  /// Returns an unsubscribe function.
  void Function() onChange(void Function(String flagKey) callback) {
    _changeListeners.add(callback);
    return () => _changeListeners.remove(callback);
  }

  void _emitChange(String flagKey) {
    // Copy first so a listener that unsubscribes during dispatch is safe.
    for (final cb in List<void Function(String)>.from(_changeListeners)) {
      try {
        cb(flagKey);
      } catch (_) {
        // A misbehaving listener must not break flag delivery.
      }
    }
  }

  /// Loads the persisted flag snapshot into the Rust store so flags can be
  /// evaluated offline before the first successful connection. Best-effort.
  Future<void> _hydrateFromCache() async {
    final s = storage;
    if (s == null) return;
    try {
      final raw = await s.read(_cacheKey);
      if (raw == null) return;
      final snapshot = jsonDecode(raw) as Map<String, dynamic>;
      snapshot.forEach((key, flag) {
        final f = flag as Map<String, dynamic>;
        _flagCache[key] = f;
        final ptr = jsonEncode(f).toNativeUtf8();
        _bindings!.checkgate_upsert_flag_v2(ptr);
        malloc.free(ptr);
      });
      if (!_ready) _hydrated = true;
    } catch (_) {
      // Corrupt/missing cache is non-fatal.
    }
  }

  /// Persists the current flag set to [storage] (best-effort, fire-and-forget).
  void _persistCache() {
    final s = storage;
    if (s == null) return;
    // Ignore write errors — analytics/cache loss must never break evaluation.
    s.write(_cacheKey, jsonEncode(_flagCache)).catchError((_) {});
  }

  bool _loopLive(int generation) => !_closed && generation == _loopGeneration;

  void _startSseLoop(int generation, http.Client client, Uri uri,
      Map<String, String> headers) async {
    while (_loopLive(generation)) {
      try {
        final request = http.Request('GET', uri);
        request.headers.addAll(headers);
        // The replay this attempt receives is read after this point.
        _streamStartedAt = _learnSeq;
        // Until `connected` arrives only the connect deadline applies; it
        // also bounds waiting for the response headers.
        _idleTimeout = Duration.zero;
        final response =
            await client.send(request).timeout(connectTimeout);
        if (!_loopLive(generation)) break;

        if (_authFailureStatuses.contains(response.statusCode)) {
          _failAuth(response.statusCode);
          break;
        }
        if (response.statusCode != 200) {
          throw Exception('SSE connect failed: ${response.statusCode}');
        }

        await for (final event in parseSseEvents(_watched(response.stream),
            onFrame: _touchStream)) {
          if (!_loopLive(generation)) break;
          _handleSseEvent(event.key, event.value);
        }
      } catch (e) {
        if (!_loopLive(generation)) break;
      } finally {
        _stopWatchdog();
      }

      // A replay cut short never replaces the live snapshot.
      _staged = null;
      _stagedRevisions = null;
      if (!_loopLive(generation)) break;
      // Stream dropped — wait with exponential backoff + jitter before retrying.
      final delay = _nextReconnectDelay();
      _state = _polling
          ? CheckgateConnectionState.polling
          : (_ready
              ? CheckgateConnectionState.reconnecting
              : CheckgateConnectionState.connecting);
      await Future<void>.delayed(delay);
    }
  }

  /// Passes [bytes] through and fails the stream with a [TimeoutException]
  /// once it misses its deadline: [connectTimeout] until `connected`, then
  /// the idle timeout, pushed out by every complete frame (SSE comments
  /// included; see [parseSseEvents]). Bytes alone do not count: a stream that
  /// trickles without ever finishing a frame is as useless as a silent one.
  /// A dropped NAT mapping or a half-open TCP connection otherwise looks
  /// healthy until the OS gives up, hours later.
  Stream<List<int>> _watched(Stream<List<int>> bytes) {
    late final StreamController<List<int>> out;
    StreamSubscription<List<int>>? sub;
    void abort(Object error) {
      sub?.cancel();
      sub = null;
      if (!out.isClosed) {
        out.addError(error);
        out.close();
      }
    }

    out = StreamController<List<int>>(
      onListen: () {
        sub = bytes.listen(
          out.add,
          onError: out.addError,
          onDone: () {
            if (!out.isClosed) out.close();
          },
        );
      },
      onCancel: () => sub?.cancel(),
    );
    _abortStream = () => abort(StateError('stream closed'));
    _armWatchdog(connectTimeout, () => abort(TimeoutException(
        'No event from the stream within its deadline', connectTimeout)));
    return out.stream;
  }

  void Function()? _onStreamDead;

  void _armWatchdog(Duration after, void Function() onDead) {
    _onStreamDead = onDead;
    _deadline = DateTime.now().add(after);
    _watchdog?.cancel();
    _watchdog = Timer(after, _checkDeadline);
  }

  void _checkDeadline() {
    _watchdog = null;
    final deadline = _deadline;
    if (deadline == null) return;
    final left = deadline.difference(DateTime.now());
    if (left > Duration.zero) {
      _watchdog = Timer(left, _checkDeadline);
      return;
    }
    _onStreamDead?.call();
  }

  /// Records that the current stream is alive and pushes its deadline out.
  void _touchStream() {
    if (_idleTimeout > Duration.zero && _deadline != null) {
      _deadline = DateTime.now().add(_idleTimeout);
    }
    if (_state == CheckgateConnectionState.streaming && _staged == null) {
      _lastSyncAt = DateTime.now();
    }
  }

  void _stopWatchdog() {
    _watchdog?.cancel();
    _watchdog = null;
    _deadline = null;
    _onStreamDead = null;
    _abortStream = null;
  }

  /// The server rejected the SDK key: stop every retry (stream and poll),
  /// keep serving the flags already loaded, and settle connect().
  void _failAuth(int statusCode) {
    final err = CheckgateAuthException(statusCode);
    _loopGeneration++; // ends the SSE loop
    _abortStream?.call();
    _stopWatchdog();
    _stopPollFallback();
    _staged = null;
    _stagedRevisions = null;
    _reconnectAttempts = 0;
    _connecting = false;
    _state = CheckgateConnectionState.authFailed;
    _lastError = err;
    final completer = _readyCompleter;
    _readyCompleter = null;
    if (completer != null && !completer.isCompleted) {
      completer.completeError(err);
    }
  }

  /// Exponential backoff with up to 30% jitter, capped at [maxReconnectDelay].
  Duration _nextReconnectDelay() {
    final attempt = _reconnectAttempts;
    _reconnectAttempts++;

    // SSE is failing repeatedly (e.g. a proxy blocking long-lived connections) —
    // fall back to polling so flags still stay current. SSE retries continue in
    // the background; a successful reconnect stops the poll loop again.
    if (_reconnectAttempts >= pollFallbackThreshold) {
      _startPollFallback();
    }

    final shift = attempt > 20 ? 20 : attempt; // guard against overflow
    final baseMs = reconnectBaseDelay.inMilliseconds << shift;
    final cappedMs = baseMs > maxReconnectDelay.inMilliseconds
        ? maxReconnectDelay.inMilliseconds
        : baseMs;
    final jitterMs = (cappedMs * 0.3 * _rng.nextDouble()).round();
    return Duration(milliseconds: cappedMs + jitterMs);
  }

  // ---------------------------------------------------------------------
  // Poll fallback (used when SSE cannot be established at all)
  // ---------------------------------------------------------------------

  /// Whether the client is currently polling `GET /flags/snapshot` instead of
  /// streaming.
  bool get isPolling => _polling;

  void _startPollFallback() {
    if (_polling) return;
    _polling = true;
    _pollSnapshot(); // poll immediately rather than waiting for the first interval tick
    _pollTimer = Timer.periodic(pollInterval, (_) => _pollSnapshot());
  }

  void _stopPollFallback() {
    // Invalidate any poll still in flight: its snapshot predates the stream
    // state about to replace it, and applying it afterwards would roll flags
    // back and reset the revisions that keep delayed events out.
    _syncGeneration++;
    if (!_polling) return;
    _polling = false;
    _pollTimer?.cancel();
    _pollTimer = null;
  }

  /// Fetches the full flag snapshot and applies it, diffing against the last
  /// known state (`_flagCache`) so only genuinely changed flags trigger
  /// onChange listeners. Resolves connect() if this is the first successful
  /// sync (SSE may never have connected at all). Best-effort: network/parse
  /// failures are silently retried on the next interval tick.
  ///
  /// At most one request is in flight (later ticks share it), each is bounded
  /// by [_pollTimeout], and a response that arrives after the stream took over
  /// (or after [close]) is discarded — the same guards the JavaScript SDKs use.
  Future<void> _pollSnapshot() =>
      _pollInFlight ??= _fetchSnapshot(_syncGeneration)
          .whenComplete(() => _pollInFlight = null);

  Future<void> _fetchSnapshot(int generation) async {
    final client = _httpClient;
    if (client == null) return;
    // The server reads the snapshot after this point (see _mergeFullSet).
    final startedAt = _learnSeq;
    try {
      // `v=2` asks for per-flag revisions and recent deletions; servers
      // before 0.1.31 ignore it and send a bare array.
      final response = await client.get(
        Uri.parse('$serverUrl/flags/snapshot?v=2'),
        headers: {'Authorization': 'Bearer $sdkKey'},
      ).timeout(_pollTimeout);
      if (generation != _syncGeneration || _closed) return;
      if (_authFailureStatuses.contains(response.statusCode)) {
        _failAuth(response.statusCode);
        return;
      }
      if (response.statusCode != 200) return;

      final snapshot = _parseSnapshot(jsonDecode(response.body));
      if (generation != _syncGeneration || _closed) return;
      _applySnapshot(snapshot.flags, snapshot.revisions, startedAt);
      _hydrated = true;
      if (_state != CheckgateConnectionState.streaming) {
        _state = CheckgateConnectionState.polling;
      }
      _markReady();
    } catch (_) {
      // Best-effort: network/parse failures are retried on the next interval tick.
    }
  }

  /// Applies a complete flag set (a stream replay or a poll snapshot), merged
  /// with what the client holds by [_mergeFullSet]: [revisions] is null when
  /// the server sent none, and [startedAt] is the learn counter when the set
  /// was requested. Then diffs against `_flagCache` so only flags that
  /// actually changed (added, updated, or removed) trigger onChange — after
  /// the whole set is applied — and a cache write. Returns true if anything
  /// changed.
  bool _applySnapshot(List<dynamic> flags,
      [Map<String, _Revision>? revisions, int? startedAt]) {
    final incoming = <String, Map<String, dynamic>>{};
    for (final raw in flags) {
      final flag = raw as Map<String, dynamic>;
      incoming[flag['key'] as String] = flag;
    }
    final merged = _mergeFullSet(_flagCache, _revisions, incoming, revisions,
        startedAt ?? _learnSeq, ++_learnSeq);
    final newKeys = <String>{};
    final changed = <String>[];

    for (final flag in merged.flags.values) {
      final key = flag['key'] as String;
      newKeys.add(key);
      final prev = _flagCache[key];
      if (prev != null && jsonEncode(prev) == jsonEncode(flag)) continue;

      _upsertFlag(flag);
      _flagCache[key] = flag;
      changed.add(key);
    }

    for (final key in List<String>.from(_flagCache.keys)) {
      if (newKeys.contains(key)) continue;
      _deleteFlag(key);
      _flagCache.remove(key);
      changed.add(key);
    }

    _revisions = merged.revisions;
    if (changed.isEmpty) return false;
    _persistCache();
    // Notify only once the whole snapshot is in place: a listener that
    // evaluates a flag must not see some of its prerequisites updated and
    // others not yet.
    if (_ready) changed.forEach(_emitChange);
    return true;
  }

  void _handleSseEvent(String eventName, String data) {
    if (eventName == 'connected') {
      // The server replays the full flag set next. Stage it and only replace
      // the live set on "ready", so evaluations keep using the last complete
      // snapshot meanwhile and a replay cut short never goes live.
      _staged = {};
      _stagedRevisions = {};
      // A successful connection resets the backoff schedule and ends any
      // active poll fallback — SSE deltas take over again.
      _reconnectAttempts = 0;
      _stopPollFallback();

      // Capture the environment id so impressions route to the right endpoint.
      // Older servers send "true" (no id) — parsing simply yields no id.
      Object? payload;
      try {
        payload = jsonDecode(data);
      } catch (_) {
        payload = null;
      }
      _envId = payload is Map<String, dynamic>
          ? payload['environment_id'] as String?
          : null;
      // From now on the stream must keep talking; see [streamIdleTimeout].
      final heartbeatMs =
          payload is Map<String, dynamic> ? payload['heartbeat_ms'] : null;
      _idleTimeout = _idleTimeoutFor(heartbeatMs);
      if (_idleTimeout > Duration.zero && _deadline != null) {
        _deadline = DateTime.now().add(_idleTimeout);
        _watchdog?.cancel();
        _watchdog = Timer(_idleTimeout, _checkDeadline);
      } else if (_idleTimeout == Duration.zero) {
        _watchdog?.cancel();
        _watchdog = null;
        _deadline = null;
      }
      _startImpressionTimer();
      return;
    }

    if (eventName == 'ready') {
      final staged = _staged;
      if (staged == null) return;
      final revisions = _stagedRevisions ?? const {};
      _staged = null;
      _stagedRevisions = null;
      // Swap in the complete replay, except where a poll that completed while
      // it was on its way is newer (R20). On a reconnect, listeners fire only
      // for flags that changed while the stream was down.
      _applySnapshot(staged.values.toList(), revisions, _streamStartedAt);
      _hydrated = true;
      _persistCache(); // save the freshly-bootstrapped full set for offline use
      _state = CheckgateConnectionState.streaming;
      _markReady();
      return;
    }

    if (eventName == 'update') {
      try {
        _applyDelta(jsonDecode(data) as Map<String, dynamic>);
      } catch (_) {
        // Ignore malformed messages.
      }
    }
  }

  /// Applies one stream change event. While a (re)connect replay is in
  /// progress the event is staged for "ready"; afterwards it is applied unless
  /// it is older than the revision the client already holds for that flag.
  void _applyDelta(Map<String, dynamic> event) {
    final type = event['type'];
    final isUpsert = type == 'UPSERT';
    if (!isUpsert && type != 'DELETE') return;
    final flag = isUpsert ? event['flag'] as Map<String, dynamic>? : null;
    final key = isUpsert ? (flag?['key']) : event['key'];
    if (key is! String) return;
    final rev = event['revision'];
    final revision = rev is int ? rev : null;

    final staged = _staged;
    if (staged != null) {
      if (_isStaleEvent(_stagedRevisions![key], revision, isUpsert: isUpsert)) {
        return;
      }
      if (isUpsert) {
        staged[key] = flag!;
      } else {
        staged.remove(key);
      }
      if (revision != null) {
        _stagedRevisions![key] = _Revision(revision, deleted: !isUpsert);
      }
      return;
    }

    if (_isStaleEvent(_revisions[key], revision, isUpsert: isUpsert)) return;
    if (revision != null) {
      _revisions[key] =
          _Revision(revision, deleted: !isUpsert, at: ++_learnSeq);
    }
    if (isUpsert) {
      _upsertFlag(flag!);
      _flagCache[key] = flag;
    } else {
      // A delete of a flag the client does not hold changes nothing.
      if (_flagCache.remove(key) == null) return;
      _deleteFlag(key);
    }
    _emitChange(key);
    _persistCache();
  }

  // Both no-op before connect() has loaded the native engine, which only
  // happens in unit tests: stream events cannot arrive before connect().
  void _upsertFlag(Map<String, dynamic> flag) {
    final bindings = _bindings;
    if (bindings == null) return;
    // Pass the full flag (flag_type, default/disabled values, and per-rule
    // variants) so getValue()/getVariant() resolve non-boolean flags correctly.
    final flagJson = jsonEncode(flag).toNativeUtf8();
    bindings.checkgate_upsert_flag_v2(flagJson);
    malloc.free(flagJson);
  }

  void _deleteFlag(String key) {
    final bindings = _bindings;
    if (bindings == null) return;
    final k = key.toNativeUtf8();
    bindings.checkgate_delete_flag(k);
    malloc.free(k);
  }

  /// Evaluate a flag for a user synchronously (in-process, no network).
  ///
  /// [attributes] is a flat map of string→string user attributes used for
  /// targeting rules (e.g. `{'email': 'u@acme.com', 'country': 'US'}`).
  bool isEnabled(
    String flagKey,
    String userKey, [
    Map<String, String> attributes = const {},
  ]) {
    if (!_ready && !_hydrated) return false;

    final fKey = flagKey.toNativeUtf8();
    final uKey = userKey.toNativeUtf8();
    final attrsJson = jsonEncode(attributes).toNativeUtf8();

    final result =
        _bindings!.checkgate_is_enabled(fKey, uKey, attrsJson);

    malloc.free(fKey);
    malloc.free(uKey);
    malloc.free(attrsJson);

    final enabled = result != 0;
    _recordImpression(flagKey, userKey, enabled ? 'true' : 'false', attributes);
    return enabled;
  }

  /// Evaluates a multi-variant flag and returns the full result
  /// `{ 'enabled': bool, 'value': ... }`, where `value` is the resolved variant
  /// (bool, String, int, or a decoded JSON object/list).
  ///
  /// Returns `null` if the flag does not exist.
  Map<String, dynamic>? getVariant(
    String flagKey,
    String userKey, [
    Map<String, String> attributes = const {},
  ]) {
    if (!_ready && !_hydrated) return null;

    final fKey = flagKey.toNativeUtf8();
    final uKey = userKey.toNativeUtf8();
    final attrsJson = jsonEncode(attributes).toNativeUtf8();

    final ptr = _bindings!.checkgate_get_variant(fKey, uKey, attrsJson);

    malloc.free(fKey);
    malloc.free(uKey);
    malloc.free(attrsJson);

    if (ptr == nullptr) return null;
    final json = ptr.toDartString();
    _bindings!.checkgate_free_string(ptr);

    final decoded = jsonDecode(json);
    if (decoded == null) return null;
    final variant = decoded as Map<String, dynamic>;
    _recordImpression(
        flagKey, userKey, _formatValue(variant['value']), attributes);
    return variant;
  }

  /// Evaluates a multi-variant flag and returns just its resolved value.
  ///
  /// [defaultValue] is returned when the flag does not exist.
  dynamic getValue(
    String flagKey,
    String userKey, [
    Map<String, String> attributes = const {},
    dynamic defaultValue,
  ]) {
    final variant = getVariant(flagKey, userKey, attributes);
    return variant == null ? defaultValue : variant['value'];
  }

  // -------------------------------------------------------------------------
  // Impression reporting (async, batched, best-effort)
  // -------------------------------------------------------------------------

  /// Serialize a resolved variant value into the string stored server-side.
  String _formatValue(dynamic value) {
    if (value == null) return 'null';
    if (value is String) return value;
    if (value is bool || value is num) return value.toString();
    return jsonEncode(value);
  }

  /// Buffer one evaluation event; flush when the batch fills up.
  void _recordImpression(
    String flagKey,
    String userKey,
    String value,
    Map<String, String> attributes,
  ) {
    if (!reportImpressions || _envId == null) return;

    _impressions.add({
      'flag_key': flagKey,
      'user_id': userKey,
      'value': value,
      // When the evaluation happened, not when the batch reaches the server:
      // impressions and conversions flush independently, and experiment
      // attribution needs the real order.
      'evaluated_at': DateTime.now().toUtc().toIso8601String(),
      // Attributes are omitted unless explicitly opted in — the whole point of
      // local evaluation is that user attributes never leave the device.
      if (sendEvaluationContext) 'context': attributes,
    });

    // Bound memory if the server is unreachable: keep only the newest events.
    if (_impressions.length > 10000) {
      _impressions.removeRange(0, _impressions.length - 10000);
    }
    if (_impressions.length >= impressionBatchSize) {
      _flushImpressions();
    }
  }

  void _startImpressionTimer() {
    // Not gated on reportImpressions: the timer also flushes track() events, and
    // both flush methods no-op on empty buffers, so an idle timer is cheap.
    if (_flushTimer != null) return;
    _flushTimer = Timer.periodic(impressionFlushInterval, (_) {
      _flushImpressions();
      _flushEvents();
    });
  }

  /// POST buffered impressions (up to the server's 500/batch limit).
  void _flushImpressions() {
    final client = _httpClient;
    if (client == null || _envId == null || _impressions.isEmpty) return;
    final take = _impressions.length > 500 ? 500 : _impressions.length;
    final batch = _impressions.sublist(0, take);
    _impressions.removeRange(0, take);

    client
        .post(
          Uri.parse('$serverUrl/api/environments/$_envId/impressions'),
          headers: {
            'Content-Type': 'application/json',
            'Authorization': 'Bearer $sdkKey',
          },
          body: jsonEncode(batch),
        )
        .catchError((Object err) {
      // Best-effort: analytics loss is acceptable, never surface to the caller.
      return http.Response('', 599);
    });
  }

  // -------------------------------------------------------------------------
  // Goal event tracking (A/B testing conversions)
  // -------------------------------------------------------------------------

  /// Records a goal/conversion event for A/B testing (e.g. "checkout_complete").
  /// Buffered and reported asynchronously, mirroring impression reporting.
  ///
  /// [eventKey] is the goal event name (must match the experiment's goal).
  /// [userKey] is the same identifier passed to [getVariant]/[isEnabled].
  /// [value] is an optional numeric payload (e.g. revenue); [context] is
  /// optional metadata stored with the event.
  void track(
    String eventKey,
    String userKey, {
    num? value,
    Map<String, dynamic>? context,
  }) {
    if (eventKey.isEmpty) return;
    if (_envId == null) return;

    _events.add({
      'event_key': eventKey,
      'user_id': userKey,
      'occurred_at': DateTime.now().toUtc().toIso8601String(),
      if (value != null) 'value': value,
      if (context != null) 'context': context,
    });

    if (_events.length > 10000) {
      _events.removeRange(0, _events.length - 10000);
    }
    // Ensure events flush even when impression auto-reporting is disabled.
    _startImpressionTimer();
    if (_events.length >= impressionBatchSize) {
      _flushEvents();
    }
  }

  /// POST buffered goal events (up to the server's 500/batch limit).
  void _flushEvents() {
    final client = _httpClient;
    if (client == null || _envId == null || _events.isEmpty) return;
    final take = _events.length > 500 ? 500 : _events.length;
    final batch = _events.sublist(0, take);
    _events.removeRange(0, take);

    client
        .post(
          Uri.parse('$serverUrl/api/environments/$_envId/events'),
          headers: {
            'Content-Type': 'application/json',
            'Authorization': 'Bearer $sdkKey',
          },
          body: jsonEncode(batch),
        )
        .catchError((Object err) {
      // Best-effort: analytics loss is acceptable, never surface to the caller.
      return http.Response('', 599);
    });
  }

  // ---------------------------------------------------------------------------
  // Test-only seams
  //
  // These expose internal reporting state so unit tests can exercise the
  // impression/event batching and HTTP paths (with an injected mock client)
  // without a live SSE connection or the native FFI library. Not part of the
  // public API — do not use in application code.
  // ---------------------------------------------------------------------------

  /// Test-only: set the environment id normally learned from the SSE stream.
  void debugSetEnvironment(String? envId) => _envId = envId;

  /// Test-only: buffered impression payloads awaiting flush.
  List<Map<String, dynamic>> get debugImpressions => _impressions;

  /// Test-only: buffered goal-event payloads awaiting flush.
  List<Map<String, dynamic>> get debugEvents => _events;

  /// Test-only: force a flush of buffered impressions and events.
  void debugFlush() {
    _flushImpressions();
    _flushEvents();
  }

  /// Test-only: the value serialization used for impression reporting.
  String debugFormatValue(dynamic value) => _formatValue(value);

  /// Test-only: feed one parsed SSE frame (event name + data) to the client.
  void debugHandleSseEvent(String eventName, String data) =>
      _handleSseEvent(eventName, data);

  /// Test-only: drop the stream mid-replay, as the SSE loop does.
  void debugStreamDropped() {
    _staged = null;
    _stagedRevisions = null;
  }

  /// Test-only: open a new stream attempt, as the SSE loop does before each
  /// request (its replay is read after this point).
  void debugStreamOpened() => _streamStartedAt = _learnSeq;

  /// Test-only: run one poll-fallback request, as the poll timer does.
  Future<void> debugPollSnapshot() => _pollSnapshot();

  /// Test-only: [connect] without loading the native engine (flags are
  /// tracked in [debugFlags] only, evaluations return defaults).
  Future<void> debugConnectWithoutEngine() => _connect(loadEngine: false);

  /// Test-only: the raw flags the client currently evaluates against.
  Map<String, Map<String, dynamic>> get debugFlags => _flagCache;

  /// Cancel the SSE stream and release the HTTP client.
  void close() {
    _closed = true;
    _loopGeneration++;
    _abortStream?.call();
    _stopWatchdog();
    _state = CheckgateConnectionState.closed;
    _stopPollFallback();
    _flushImpressions();
    _flushEvents();
    _flushTimer?.cancel();
    _flushTimer = null;
    _httpClient?.close();
    _httpClient = null;
    // Reset readiness so a later connect() re-establishes the stream.
    _ready = false;
    _hydrated = false;
    _staged = null;
    _stagedRevisions = null;
    _connecting = false;
    // A connect() still waiting would otherwise never complete.
    final completer = _readyCompleter;
    _readyCompleter = null;
    if (completer != null && !completer.isCompleted) {
      completer.completeError(
          StateError('close() was called before connect() completed.'));
    }
    // Reset backoff state so a later connect() doesn't inherit a stale attempt
    // count from this session (which could trigger poll fallback prematurely).
    _reconnectAttempts = 0;
  }
}

extension on CheckgateClient {
  /// How long a connected stream may stay silent: an explicit
  /// [CheckgateClient.streamIdleTimeout], else [_idleHeartbeats] times the
  /// heartbeat interval the server announced, else (servers before 0.1.31,
  /// whose 15 s keep-alive comments this client does see) the same multiple
  /// of 15 s.
  Duration _idleTimeoutFor(Object? heartbeatMs) {
    final override = streamIdleTimeout;
    if (override != null) return override;
    final interval = heartbeatMs is int && heartbeatMs > 0
        ? Duration(milliseconds: heartbeatMs)
        : _legacyKeepAlive;
    return interval * _idleHeartbeats;
  }
}

/// The revision of the last change applied to a flag, whether that change
/// deleted it, and the learn counter value when the client applied it.
class _Revision {
  const _Revision(this.revision, {required this.deleted, this.at = 0});

  final int revision;
  final bool deleted;
  final int at;

  /// Whether this version supersedes [other]. Revisions come from one
  /// server-wide sequence and grow with every committed write to a flag. A
  /// delete carries the revision of the version it removed, so it supersedes
  /// that version, and anything newer supersedes both.
  bool supersedes(_Revision other) =>
      revision > other.revision ||
      (revision == other.revision && deleted && !other.deleted);
}

/// Whether a change event is stale: not newer than what the client already
/// holds for that flag. Change events carry the flag row's revision and can
/// arrive late or twice; a duplicate is stale too, so it changes nothing and
/// notifies nobody. Events from servers that predate revisions always apply.
bool _isStaleEvent(_Revision? known, int? revision, {required bool isUpsert}) {
  if (known == null || revision == null) return false;
  return !_Revision(revision, deleted: !isUpsert).supersedes(known);
}

/// A flag set and its revisions, or null revisions when the server sent none.
class _FullSet {
  const _FullSet(this.flags, this.revisions);

  final Map<String, Map<String, dynamic>> flags;
  final Map<String, _Revision> revisions;
}

/// The state after a complete flag set: a stream replay on "ready", or a poll
/// snapshot. See docs/releases/v0.1.31/CONFIGURATION-CONSISTENCY.md.
///
/// The server read [incoming] after the client sent the request, when the
/// client's learn counter stood at [startedAt]. Everything the client learned
/// before that is older than the set, so the set replaces it, absences
/// included. What it learned afterwards (a poll that completed while a delayed
/// replay was still on its way: R20) may be newer, so for those flags the
/// newer version wins, and the set never brings back an older one.
///
/// [incomingRevisions] (deletions included) is null for a poll answered by a
/// server older than 0.1.31, which sends no revisions: the set then replaces
/// everything, exactly as before.
_FullSet _mergeFullSet(
    Map<String, Map<String, dynamic>> flags,
    Map<String, _Revision> revisions,
    Map<String, Map<String, dynamic>> incoming,
    Map<String, _Revision>? incomingRevisions,
    int startedAt,
    int at) {
  if (incomingRevisions == null) {
    return _FullSet(Map.of(incoming), {});
  }
  final outFlags = <String, Map<String, dynamic>>{};
  final outRevisions = <String, _Revision>{};
  final keys = {...incoming.keys, ...incomingRevisions.keys, ...revisions.keys};
  for (final key in keys) {
    final held = revisions[key];
    final offered = incomingRevisions[key];
    if (held != null &&
        held.at > startedAt &&
        (offered == null || !offered.supersedes(held))) {
      outRevisions[key] = held;
      final flag = flags[key];
      if (!held.deleted && flag != null) outFlags[key] = flag;
      continue;
    }
    if (offered != null) {
      outRevisions[key] =
          _Revision(offered.revision, deleted: offered.deleted, at: at);
    }
    final flag = incoming[key];
    if (flag != null && !(offered?.deleted ?? false)) outFlags[key] = flag;
  }
  return _FullSet(outFlags, outRevisions);
}

/// A parsed `GET /flags/snapshot` body.
class _Snapshot {
  const _Snapshot(this.flags, this.revisions);

  final List<dynamic> flags;
  final Map<String, _Revision>? revisions;
}

/// Reads a `GET /flags/snapshot` body: `{v: 2, flags, revisions, tombstones}`
/// from a server that understands `?v=2` (0.1.31+), or a bare list of flags
/// from an older one (null revisions). Throws [FormatException] on a malformed
/// body, so a bad response never replaces the flags the client holds.
_Snapshot _parseSnapshot(Object? body) {
  if (body is List<dynamic>) return _Snapshot(body, null);
  if (body is! Map<String, dynamic> || body['flags'] is! List<dynamic>) {
    throw const FormatException('Snapshot was not a flag set.');
  }
  final flags = body['flags'] as List<dynamic>;
  final revs = body['revisions'];
  final revisions = <String, _Revision>{};
  for (final raw in flags) {
    final key = raw is Map<String, dynamic> ? raw['key'] : null;
    final revision = revs is Map<String, dynamic> ? revs[key] : null;
    if (key is! String || revision is! int) {
      throw const FormatException('Snapshot flag without a revision.');
    }
    revisions[key] = _Revision(revision, deleted: false);
  }
  final tombstones = body['tombstones'];
  if (tombstones is Map<String, dynamic>) {
    tombstones.forEach((key, revision) {
      if (revision is! int) {
        throw const FormatException('Snapshot tombstone without a revision.');
      }
      revisions.putIfAbsent(key, () => _Revision(revision, deleted: true));
    });
  }
  return _Snapshot(flags, revisions);
}
