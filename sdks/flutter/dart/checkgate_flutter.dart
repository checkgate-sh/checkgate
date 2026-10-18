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

  // Readiness: completed once the server has replayed the full flag set and
  // sent the "ready" event.
  bool _ready = false;
  bool _connecting = false;
  Completer<void>? _readyCompleter;

  // During a (re)connect replay: the flags received so far and their
  // revisions, swapped in as a whole on "ready" (see _handleSseEvent).
  Map<String, Map<String, dynamic>>? _staged;
  Map<String, _Revision>? _stagedRevisions;
  // Last applied revision per flag key.
  Map<String, _Revision> _revisions = {};
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
    http.Client? httpClient,
  }) {
    // Test seam: an injected client lets unit tests exercise the
    // impression/event reporting paths with a mock, without a real connection.
    _httpClient = httpClient;
  }

  /// Open the SSE stream. The returned future completes once the server has
  /// replayed the full flag set and sent the "ready" event — consistent with
  /// the Node and Web SDKs. The server sends the full flag state on connect,
  /// so no separate REST bootstrap call is needed.
  Future<void> connect() {
    if (_ready) return Future<void>.value();
    if (_connecting) return _readyCompleter!.future;

    _connecting = true;
    _closed = false;
    _readyCompleter = Completer<void>();
    // Load the native engine lazily (once), and open an HTTP client unless one
    // was injected for testing.
    _bindings ??= CheckgateBindings.open();
    _httpClient ??= http.Client();
    // Hydrate from the persisted snapshot so evaluations work offline
    // immediately, then open the live stream.
    _hydrateFromCache().whenComplete(() {
      if (!_closed) {
        _startSseLoop(_httpClient!, Uri.parse('$serverUrl/stream'), {
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

  void _startSseLoop(
      http.Client client, Uri uri, Map<String, String> headers) async {
    while (!_closed) {
      try {
        final request = http.Request('GET', uri);
        request.headers.addAll(headers);
        final response = await client.send(request);

        if (response.statusCode != 200) {
          throw Exception('SSE connect failed: ${response.statusCode}');
        }

        String eventName = '';
        final buffer = StringBuffer();

        await for (final chunk
            in response.stream.transform(utf8.decoder)) {
          if (_closed) break;
          for (final line in chunk.split('\n')) {
            if (line.startsWith('event:')) {
              eventName = line.substring(6).trim();
            } else if (line.startsWith('data:')) {
              buffer.write(line.substring(5).trim());
            } else if (line.isEmpty && buffer.isNotEmpty) {
              final data = buffer.toString();
              buffer.clear();
              _handleSseEvent(eventName, data);
              eventName = '';
            }
          }
        }
      } catch (e) {
        if (_closed) break;
      }

      // A replay cut short never replaces the live snapshot.
      _staged = null;
      _stagedRevisions = null;
      if (_closed) break;
      // Stream dropped — wait with exponential backoff + jitter before retrying.
      await Future<void>.delayed(_nextReconnectDelay());
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
  Future<void> _pollSnapshot() async {
    final client = _httpClient;
    if (client == null) return;
    try {
      final response = await client.get(
        Uri.parse('$serverUrl/flags/snapshot'),
        headers: {'Authorization': 'Bearer $sdkKey'},
      );
      if (response.statusCode != 200) return;

      final flags = jsonDecode(response.body) as List<dynamic>;
      _applySnapshot(flags);
      _hydrated = true;

      if (!_ready) {
        _ready = true;
        final completer = _readyCompleter;
        if (completer != null && !completer.isCompleted) {
          completer.complete();
        }
      }
    } catch (_) {
      // Best-effort: network/parse failures are retried on the next interval tick.
    }
  }

  /// Applies a full flag snapshot, diffing against `_flagCache` so only flags
  /// that actually changed (added, updated, or removed) trigger onChange and a
  /// cache write. [revisions] replaces the per-flag revisions; poll snapshots
  /// carry none. Returns true if anything changed.
  bool _applySnapshot(List<dynamic> flags,
      [Map<String, _Revision> revisions = const {}]) {
    final newKeys = <String>{};
    var anyChange = false;

    for (final raw in flags) {
      final flag = raw as Map<String, dynamic>;
      final key = flag['key'] as String;
      newKeys.add(key);
      final prev = _flagCache[key];
      if (prev != null && jsonEncode(prev) == jsonEncode(flag)) continue;

      _upsertFlag(flag);
      _flagCache[key] = flag;
      anyChange = true;
      if (_ready) _emitChange(key);
    }

    for (final key in List<String>.from(_flagCache.keys)) {
      if (newKeys.contains(key)) continue;
      _deleteFlag(key);
      _flagCache.remove(key);
      anyChange = true;
      if (_ready) _emitChange(key);
    }

    _revisions = Map<String, _Revision>.of(revisions);
    if (anyChange) _persistCache();
    return anyChange;
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
      try {
        final payload = jsonDecode(data);
        _envId = payload is Map<String, dynamic>
            ? payload['environment_id'] as String?
            : null;
      } catch (_) {
        _envId = null;
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
      // Swap in the complete replay. On a reconnect, listeners fire only for
      // flags that changed while the stream was down.
      _applySnapshot(staged.values.toList(), revisions);
      _hydrated = true;
      _persistCache(); // save the freshly-bootstrapped full set for offline use
      _ready = true;
      final completer = _readyCompleter;
      if (completer != null && !completer.isCompleted) {
        completer.complete();
      }
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
      _revisions[key] = _Revision(revision, deleted: !isUpsert);
    }
    if (isUpsert) {
      _upsertFlag(flag!);
      _flagCache[key] = flag;
    } else {
      _deleteFlag(key);
      _flagCache.remove(key);
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

  /// Evaluate a flag for a user synchronously (sub-microsecond, no network).
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

  /// Test-only: the raw flags the client currently evaluates against.
  Map<String, Map<String, dynamic>> get debugFlags => _flagCache;

  /// Cancel the SSE stream and release the HTTP client.
  void close() {
    _closed = true;
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
    _readyCompleter = null;
    // Reset backoff state so a later connect() doesn't inherit a stale attempt
    // count from this session (which could trigger poll fallback prematurely).
    _reconnectAttempts = 0;
  }
}

/// The revision of the last change applied to a flag, and whether that change
/// deleted it.
class _Revision {
  const _Revision(this.revision, {required this.deleted});

  final int revision;
  final bool deleted;
}

/// Whether a change event is older than what the client already holds for
/// that flag. Change events carry the flag row's revision and can arrive out
/// of order; events from servers that predate revisions always apply. After a
/// delete, only a strictly newer upsert may bring the flag back.
bool _isStaleEvent(_Revision? known, int? revision, {required bool isUpsert}) {
  if (known == null || revision == null) return false;
  if (isUpsert && known.deleted) return revision <= known.revision;
  return revision < known.revision;
}
