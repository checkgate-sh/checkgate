const { cacheNamespace } = require('./cache-key.js');
// `native-binding.js` is the napi-rs-generated multi-platform dispatch loader
// (built via `--js native-binding.js`, kept out of `index.js` so it doesn't
// collide with this hand-written wrapper) — it picks the correct
// `checkgate.<platform>.node` for the current OS/arch/libc at require time.
const { CheckgateCore } = require('./native-binding.js');
// eventsource v4 exports EventSource as a named export, not a default export.
const { EventSource } = require('eventsource');

/**
 * Whether flag version `a` supersedes version `b` (both `{ revision, deleted }`).
 * Revisions come from one server-wide sequence and grow with every committed
 * write to a flag. A delete carries the revision of the version it removed, so
 * it supersedes that version, and anything newer supersedes both.
 */
function supersedes(a, b) {
    return a.revision > b.revision || (a.revision === b.revision && a.deleted && !b.deleted);
}

/**
 * Whether a change event is stale: not newer than what the client already
 * holds for that flag. Change events carry the flag row's revision and can
 * arrive late or twice; a duplicate is stale too, so it changes nothing and
 * notifies nobody. Events from servers that predate revisions (no number)
 * always apply.
 */
function isStaleEvent(known, revision, isUpsert) {
    if (!known || typeof revision !== 'number') return false;
    return !supersedes({ revision, deleted: !isUpsert }, known);
}

/**
 * The state after a complete flag set: a stream replay on "ready", or a poll
 * snapshot. See docs/operations/configuration-consistency.md.
 *
 * The server read `incoming` (key → flag) after the client sent the request,
 * when the client's learn counter stood at `startedAt`. Everything the client
 * learned before that is older than the set, so the set replaces it, absences
 * included. What it learned afterwards (a poll that completed while a delayed
 * replay was still on its way: R20) may be newer, so for those flags the newer
 * version wins, and the set never brings back an older one.
 *
 * `incomingRevisions` (key → { revision, deleted }, deletions included) is null
 * for a poll answered by a server older than 0.1.31, which sends no revisions:
 * the set then replaces everything, exactly as before.
 *
 * Returns `{ flags, revisions, ignored }`; revision entries carry `at`, the
 * learn counter value when the client learned them, and `ignored` counts the
 * set's entries that were older than what the client held.
 */
function mergeFullSet(flags, revisions, incoming, incomingRevisions, startedAt, at) {
    if (!incomingRevisions) return { flags: new Map(incoming), revisions: new Map(), ignored: 0 };
    const outFlags = new Map();
    const outRevisions = new Map();
    let ignored = 0;
    const visit = (key) => {
        const held = revisions.get(key);
        const offered = incomingRevisions.get(key);
        if (held && held.at > startedAt && (!offered || !supersedes(offered, held))) {
            if (offered) ignored++;
            outRevisions.set(key, held);
            if (!held.deleted && flags.has(key)) outFlags.set(key, flags.get(key));
            return;
        }
        if (offered) {
            // The set's own entry, owned by the client from here on.
            offered.at = at;
            outRevisions.set(key, offered);
        }
        if (incoming.has(key) && !(offered && offered.deleted)) outFlags.set(key, incoming.get(key));
    };
    // Every key of the set and of what the client holds, once each.
    for (const key of incoming.keys()) visit(key);
    for (const key of incomingRevisions.keys()) if (!incoming.has(key)) visit(key);
    for (const key of revisions.keys()) if (!incoming.has(key) && !incomingRevisions.has(key)) visit(key);
    return { flags: outFlags, revisions: outRevisions, ignored };
}

/**
 * Reads a `GET /flags/snapshot` body: `{ v: 2, flags, revisions, tombstones }`
 * from a server that understands `?v=2` (0.1.31+), or a bare array of flags
 * from an older one (`revisions` null). Throws on a malformed body, so a bad
 * response never replaces the flags the client holds.
 */
function parseSnapshot(body) {
    if (Array.isArray(body)) return { flags: body, revisions: null };
    if (!body || !Array.isArray(body.flags)) throw new Error('Snapshot was not a flag set.');
    const own = (obj, key) => obj !== null && typeof obj === 'object' && Object.prototype.hasOwnProperty.call(obj, key);
    const revisions = new Map();
    for (const flag of body.flags) {
        const key = flag && flag.key;
        if (typeof key !== 'string' || !own(body.revisions, key) || !Number.isSafeInteger(body.revisions[key])) {
            throw new Error('Snapshot flag without a revision.');
        }
        revisions.set(key, { revision: body.revisions[key], deleted: false });
    }
    const tombstones = body.tombstones && typeof body.tombstones === 'object' ? body.tombstones : {};
    for (const key of Object.keys(tombstones)) {
        if (!Number.isSafeInteger(tombstones[key])) throw new Error('Snapshot tombstone without a revision.');
        if (!revisions.has(key)) revisions.set(key, { revision: tombstones[key], deleted: true });
    }
    return { flags: body.flags, revisions };
}

/** The engine's explanation (snake_case JSON) in the SDK's camelCase shape. */
function toDetails(raw) {
    const out = { enabled: raw.enabled, value: raw.value, reason: raw.reason };
    if (raw.rule_index !== undefined) out.ruleIndex = raw.rule_index;
    if (raw.rollout_bucket !== undefined) out.rolloutBucket = raw.rollout_bucket;
    if (raw.variant_index !== undefined) out.variantIndex = raw.variant_index;
    if (Array.isArray(raw.prerequisites)) {
        out.prerequisites = raw.prerequisites.map((p) => {
            const check = { flagKey: p.flag_key, outcome: p.outcome };
            if (p.required_value !== undefined) check.requiredValue = p.required_value;
            if (p.result !== undefined) check.result = p.result;
            return check;
        });
    }
    return out;
}

/** HTTP statuses that mean the SDK key itself is rejected: retrying cannot help. */
const AUTH_FAILURE_STATUSES = new Set([401, 403]);

/**
 * A healthy stream is never silent for longer than the server's heartbeat
 * interval; it is declared dead after this many intervals without an event.
 */
const IDLE_HEARTBEATS = 3;

/**
 * Raised (as the `connect()` rejection and in `getStatus().error`) when the
 * server rejects the SDK key with HTTP 401 or 403. The client stops retrying:
 * a rejected key does not become valid by asking again. Flags already loaded
 * keep being served. Call `connect()` again to retry, e.g. after the key was
 * restored.
 */
class CheckgateAuthError extends Error {
    constructor(status) {
        super(`[Checkgate] The server rejected the SDK key (HTTP ${status}). ` +
            'Not retrying; check the key, then call connect() again.');
        this.name = 'CheckgateAuthError';
        this.status = status;
    }
}

/** The HTTP status carried by a stream error event, if it carries one. */
function errorStatus(event) {
    const code = event && event.code;
    return typeof code === 'number' ? code : undefined;
}

/**
 * How long a connected stream may stay silent before it is considered dead:
 * an explicit `streamIdleTimeoutMs` (0 disables), else IDLE_HEARTBEATS times
 * the heartbeat interval the server announced. Servers before 0.1.31 announce
 * none and send no heartbeat events, so a quiet stream is not timed out.
 */
function idleTimeoutFor(heartbeatMs, override) {
    if (typeof override === 'number' && override >= 0) return override;
    return typeof heartbeatMs === 'number' && heartbeatMs > 0 ? heartbeatMs * IDLE_HEARTBEATS : 0;
}

class CheckgateClient {
    /**
     * @param {Object} options
     * @param {string} options.serverUrl - Base URL of your Checkgate server
     * @param {string} [options.sdkKey] - SDK key for authentication
     * @param {number} [options.reconnectDelayMs=3000] - Base SSE reconnect delay in ms (grows exponentially)
     * @param {number} [options.maxReconnectDelayMs=30000] - Maximum backoff delay between reconnects
     * @param {number} [options.pollFallbackThreshold=3] - Consecutive failed SSE reconnects
     *        before falling back to polling GET /flags/snapshot. Set to Infinity to disable.
     * @param {number} [options.pollIntervalMs=30000] - Poll interval while in fallback mode.
     * @param {number} [options.connectTimeoutMs=30000] - A stream attempt that has not
     *        received the server's "connected" event within this time is abandoned and retried.
     * @param {number} [options.streamIdleTimeoutMs] - A connected stream with no event
     *        (including heartbeats) for this long is treated as dead and reconnected.
     *        Default: 3 × the heartbeat interval the server announces (45 s by default);
     *        no idle timeout against servers older than 0.1.31. `0` disables it.
     * @param {boolean} [options.reportImpressions=true] - Report evaluation events for analytics
     * @param {number} [options.impressionBatchSize=50] - Flush after this many buffered events
     * @param {number} [options.impressionFlushIntervalMs=10000] - Periodic flush interval
     * @param {boolean} [options.sendEvaluationContext=false] - Include user attributes in
     *        reported impressions. Off by default so attributes never leave the process.
     * @param {{ getItem(key: string): string|null|Promise<string|null>,
     *           setItem(key: string, value: string): void|Promise<void> }} [options.storage]
     *        Optional persistence adapter. When provided, the last flag snapshot is saved
     *        and re-hydrated on the next start so flags evaluate before/without a connection.
     */
    constructor({
        serverUrl,
        sdkKey,
        reconnectDelayMs = 3000,
        maxReconnectDelayMs = 30000,
        pollFallbackThreshold = 3,
        pollIntervalMs = 30000,
        connectTimeoutMs = 30000,
        streamIdleTimeoutMs = undefined,
        reportImpressions = true,
        impressionBatchSize = 50,
        impressionFlushIntervalMs = 10000,
        sendEvaluationContext = false,
        storage = null,
    } = {}) {
        this.serverUrl = serverUrl;
        this.sdkKey = sdkKey;
        this.reconnectDelayMs = reconnectDelayMs;
        this.maxReconnectDelayMs = maxReconnectDelayMs;
        this.core = new CheckgateCore();
        this._ready = false;
        // During a (re)connect replay: the flags received so far and their
        // revisions. Swapped in as a whole on "ready" (see _connectDeltas).
        this._staged = null;
        this._stagedRevisions = null;
        // Last applied revision per flag key: { revision, deleted, at }, where
        // `at` is the learn counter value when it was applied. The counter
        // grows with every applied change; a full set compares it with the
        // value at its request (see mergeFullSet). Never a clock.
        this._revisions = new Map();
        this._learnSeq = 0;
        this._streamStartedAt = 0;
        this._connectPromise = null;
        this._resolveReady = null;
        this._rejectReady = null;
        this._changeListeners = [];
        this._statusListeners = [];
        this.sse = null;

        // Diagnostics (see getHealth()). Counters and timestamps only: never
        // the SDK key, user keys or attributes.
        this._source = 'none';
        this._lastReceivedRevision = null;
        this._staleEventsIgnored = 0;
        this._reconnectCount = 0;
        this._lastPollAt = null;
        this._lastSyncError = null;
        this._hydratedFromCache = false;
        this._lastPersistAt = null;

        // Connection health (see getStatus()). The watchdog closes a stream
        // that has gone silent: a dropped NAT mapping or a half-open TCP
        // connection otherwise looks healthy until the OS gives up (hours).
        this.connectTimeoutMs = connectTimeoutMs;
        this.streamIdleTimeoutMs = streamIdleTimeoutMs;
        this._state = 'idle';
        this._lastSyncAt = null;
        this._lastError = null;
        this._idleTimeoutMs = 0;
        this._watchdog = null;
        this._watchdogFireAt = 0;
        this._watchdogDeadline = 0;

        // Offline persistence: shadow copy of the raw flag set, persisted to
        // `storage` on bootstrap and re-hydrated on the next cold start so flags
        // can be evaluated before (or without) a live connection.
        this.storage = storage;
        this._flagCache = new Map();
        this._hydrated = false;
        this._cacheKey = `checkgate:flags:v2:${serverUrl}:${cacheNamespace(sdkKey || 'session')}`;

        // Managed-reconnect state (exponential backoff with jitter)
        this._reconnectAttempts = 0;
        this._reconnectTimer = null;
        this._userClosed = false;

        // Poll fallback: used when SSE cannot be established at all (e.g. a proxy
        // or firewall blocking long-lived connections). Reuses _flagCache to diff
        // each poll response against the last known state.
        this.pollFallbackThreshold = pollFallbackThreshold;
        this.pollIntervalMs = pollIntervalMs;
        this._polling = false;
        this._pollTimer = null;
        this._syncGeneration = 0;
        this._pollPromise = null;
        this._pollAbort = null;

        // Impression reporting state
        this.reportImpressions = reportImpressions;
        this.impressionBatchSize = impressionBatchSize;
        this.impressionFlushIntervalMs = impressionFlushIntervalMs;
        this.sendEvaluationContext = sendEvaluationContext;
        this._envId = null;
        this._impressions = [];
        // Buffered goal/conversion events reported via track() (A/B testing).
        this._events = [];
        this._flushTimer = null;
    }

    /**
     * Connects to the server's SSE stream and downloads the current flag set.
     * Resolves once the initial bootstrap is complete — i.e. the server has
     * replayed the full flag set and sent the "ready" event.
     *
     * Sets up automatic reconnection on connection loss. Calling connect()
     * again while a connection is in progress returns the same pending promise.
     *
     * Rejects with a `CheckgateAuthError` if the server rejects the SDK key
     * (HTTP 401/403), and with an error if disconnect() is called first. While
     * the server is unreachable it keeps retrying; race it with a timeout if
     * the caller cannot wait. Flags already loaded keep being served either way.
     *
     * @returns {Promise<void>}
     */
    connect() {
        if (this._ready && this._state !== 'auth_failed') return Promise.resolve();
        if (this._connectPromise) return this._connectPromise;

        this._userClosed = false;
        this._lastError = null;
        // The promise exists before listeners hear about the new state, so a
        // listener that calls disconnect() settles it (rejects) instead of
        // leaving it pending forever.
        const connecting = new Promise((resolve, reject) => {
            this._resolveReady = resolve;
            this._rejectReady = reject;
        });
        this._connectPromise = connecting;
        // A caller that does not await connect() must not get an unhandled
        // rejection (which ends a Node process); one that awaits still sees it.
        connecting.catch(() => {});
        this._setState(this._ready ? 'reconnecting' : 'connecting');
        if (this._userClosed) return connecting;
        // Hydrate from the persisted snapshot first so evaluations work offline
        // immediately, then open the live stream.
        this._hydrateFromCache().finally(() => {
            if (!this._userClosed) this._connectDeltas();
        });
        return connecting;
    }

    /**
     * Whether the initial bootstrap has completed and flags are ready to evaluate.
     * @returns {boolean}
     */
    isReady() {
        return this._ready;
    }

    /**
     * Connection health, for detecting stale flags.
     *
     * - `state`: `'idle'` (never connected), `'connecting'`, `'streaming'`
     *   (live stream, flags current), `'reconnecting'`, `'polling'` (stream
     *   unavailable, polling the snapshot), `'auth_failed'` (key rejected; not
     *   retrying) or `'closed'` (after disconnect()).
     * - `lastSyncAt`: when the client last confirmed its flags are current
     *   (a completed bootstrap or poll, or any event on a live stream,
     *   heartbeats included), in ms since the epoch; `null` before the first.
     * - `error`: the error behind `'auth_failed'`, otherwise `null`.
     *
     * Flags keep being served from the last known state in every state; how
     * old they may be is `Date.now() - lastSyncAt`.
     *
     * @returns {{ state: string, lastSyncAt: number|null, error: Error|null }}
     */
    getStatus() {
        return { state: this._state, lastSyncAt: this._lastSyncAt, error: this._lastError };
    }

    /**
     * Diagnostics for health checks and dashboards (schema 1, shared by every
     * Checkgate SDK). Local state only: it does not contact the server, and
     * it never contains the SDK key, user keys or attributes.
     *
     * @returns {object} see `CheckgateHealth` in index.d.ts.
     */
    getHealth() {
        const now = Date.now();
        let lastAppliedRevision = null;
        for (const entry of this._revisions.values()) {
            if (lastAppliedRevision === null || entry.revision > lastAppliedRevision) {
                lastAppliedRevision = entry.revision;
            }
        }
        const err = this._lastSyncError;
        return {
            schemaVersion: 1,
            state: this._state,
            ready: this._ready,
            source: this._source,
            flagCount: this._flagCache.size,
            lastSyncAt: this._lastSyncAt,
            configAgeMs: this._lastSyncAt === null ? null : Math.max(0, now - this._lastSyncAt),
            lastReceivedRevision: this._lastReceivedRevision,
            lastAppliedRevision,
            staleEventsIgnored: this._staleEventsIgnored,
            reconnectAttempts: this._reconnectAttempts,
            reconnectCount: this._reconnectCount,
            polling: this._polling,
            lastPollAt: this._lastPollAt,
            lastError: err ? { message: err.message, kind: err.kind, at: err.at } : null,
            offlineCache: {
                configured: !!this.storage,
                hydrated: this._hydratedFromCache,
                lastPersistAt: this._lastPersistAt,
            },
        };
    }

    /** Records a synchronization failure for getHealth() (never secrets). */
    _noteSyncError(kind, message) {
        this._lastSyncError = { kind, message: String(message), at: Date.now() };
    }

    /**
     * A poll failed: record it, and tell status listeners while polling — the
     * state stays 'polling', so without this nobody would learn that the
     * flags stopped being refreshed (see onStatusChange).
     */
    _pollFailed(kind, message) {
        this._noteSyncError(kind, message);
        if (this._state === 'polling') this._notifyStatus();
    }

    /** Records a revision seen on the wire, applied or not. */
    _noteRevision(revision) {
        if (typeof revision === 'number' &&
            (this._lastReceivedRevision === null || revision > this._lastReceivedRevision)) {
            this._lastReceivedRevision = revision;
        }
    }

    _markReady() {
        this._lastSyncAt = Date.now();
        this._ready = true;
        // While polling, a successful poll is the only sign the flags are
        // current again (the state stays 'polling'), so report it.
        if (this._state === 'polling') this._notifyStatus();
        // Also settles a connect() issued after an auth failure, when the
        // client was already ready once.
        const resolve = this._resolveReady;
        this._resolveReady = null;
        this._rejectReady = null;
        if (resolve) resolve();
    }

    /**
     * Registers a listener invoked whenever a flag changes after the initial
     * bootstrap (created, updated, or deleted). The flag's key is passed to the
     * callback. The initial bootstrap does not trigger listeners; a reconnect
     * re-sync triggers them only for flags that changed while disconnected.
     *
     * @param {(flagKey: string) => void} callback
     * @returns {() => void} an unsubscribe function.
     */
    onChange(callback) {
        this._changeListeners.push(callback);
        return () => {
            this._changeListeners = this._changeListeners.filter((cb) => cb !== callback);
        };
    }

    _emitChange(flagKey) {
        for (const cb of this._changeListeners) {
            try {
                cb(flagKey);
            } catch (err) {
                console.error('[Checkgate] change listener threw:', err);
            }
        }
    }

    /**
     * Registers a listener invoked with `getStatus()` whenever the connection
     * state changes (e.g. `'streaming'` → `'reconnecting'`), and after each
     * poll while in poll fallback, successful or not (`'polling'` stays the
     * state; `getHealth().lastPollAt` says when one last succeeded). Not
     * called for heartbeats or individual flag changes (see onChange).
     *
     * @param {(status: { state: string, lastSyncAt: number|null, error: Error|null }) => void} callback
     * @returns {() => void} an unsubscribe function.
     */
    onStatusChange(callback) {
        this._statusListeners.push(callback);
        return () => {
            this._statusListeners = this._statusListeners.filter((cb) => cb !== callback);
        };
    }

    _setState(state) {
        if (this._state === state) return;
        this._state = state;
        this._notifyStatus();
    }

    _notifyStatus() {
        if (this._statusListeners.length === 0) return;
        const status = this.getStatus();
        for (const cb of this._statusListeners) {
            try {
                cb(status);
            } catch (err) {
                console.error('[Checkgate] status listener threw:', err);
            }
        }
    }

    _connectDeltas() {
        this._closeStream();
        // The replay this stream sends is read after this point.
        this._streamStartedAt = this._learnSeq;

        // eventsource v4 dropped the plain `headers` option in favor of a custom
        // `fetch` hook — passing `headers` directly (as older versions accepted)
        // is silently ignored, so the Bearer token would never actually be sent.
        const es = new EventSource(`${this.serverUrl}/stream`, {
            // Called for every connection attempt, including any retry the
            // library makes itself: each one gets a freshly read replay.
            fetch: (url, init) => {
                this._streamStartedAt = this._learnSeq;
                return fetch(url, {
                    ...init,
                    headers: {
                        ...init.headers,
                        ...(this.sdkKey ? { 'Authorization': `Bearer ${this.sdkKey}` } : {}),
                    },
                });
            },
        });
        this.sse = es;
        let connected = false;
        // Until "connected" arrives, only the connect deadline applies.
        this._idleTimeoutMs = 0;
        this._armWatchdog(this.connectTimeoutMs);

        // Every handler ignores events from a stream that has been replaced or
        // closed, and counts any event as proof that this one is alive.
        const on = (name, handler) => es.addEventListener(name, (e) => {
            if (this.sse !== es) return;
            this._touch();
            handler(e);
        });

        // Server sends "connected" before the full-state dump on every (re)connect.
        // The replay is staged and only replaces the live flag set on "ready", so
        // evaluations keep using the last complete snapshot while it streams in,
        // and a replay cut short by a dropped connection never goes live.
        on('connected', (e) => {
            connected = true;
            this._staged = new Map();
            this._stagedRevisions = new Map();
            // A successful connection resets the backoff schedule and ends any
            // active poll fallback — SSE deltas take over again.
            this._reconnectAttempts = 0;
            this._stopPollFallback();
            console.log('[Checkgate] Stream connected — rebuilding from server state.');

            // Capture the environment id so impressions route to the right endpoint.
            // Older servers send "true" (no id) — parsing simply yields no id.
            let payload = null;
            try {
                payload = JSON.parse(e.data);
            } catch {
                payload = null;
            }
            this._envId = payload && payload.environment_id ? payload.environment_id : null;
            // From now on the stream must keep talking: the server sends a
            // heartbeat whenever it has been quiet for `heartbeat_ms`.
            this._idleTimeoutMs = idleTimeoutFor(payload && payload.heartbeat_ms, this.streamIdleTimeoutMs);
            if (this._idleTimeoutMs > 0) this._armWatchdog(this._idleTimeoutMs);
            else this._disarmWatchdog();
            this._startImpressionTimer();
        });

        // Liveness only; _touch() has already recorded it.
        on('heartbeat', () => {});

        // Server sends "ready" once the full flag set has been replayed.
        on('ready', () => {
            const staged = this._staged;
            if (!staged) return;
            const revisions = this._stagedRevisions;
            this._staged = null;
            this._stagedRevisions = null;
            // Swap in the complete replay, except where a poll that completed
            // while it was on its way is newer (R20). On a reconnect, listeners
            // fire only for flags that changed while the stream was down.
            this._applySnapshot(staged, revisions, this._streamStartedAt);
            this._hydrated = true;
            this._source = 'server';
            // Persist the freshly-bootstrapped full flag set for offline use.
            this._persistCache();
            // Ready before the state change, so status listeners already see
            // isReady() on a first connection.
            this._markReady();
            this._setState('streaming');
        });

        on('update', (e) => {
            let event;
            try {
                event = JSON.parse(e.data);
            } catch (err) {
                console.error('[Checkgate] Failed to parse delta update:', err);
                this._noteSyncError('parse', 'A stream event could not be parsed.');
                return;
            }
            this._applyDelta(event);
        });

        es.addEventListener('error', (e) => {
            if (this.sse !== es) return;
            const status = errorStatus(e);
            if (AUTH_FAILURE_STATUSES.has(status)) {
                this._failAuth(status);
                return;
            }
            if (typeof status === 'number') this._noteSyncError('http', `Stream request failed: HTTP ${status}`);
            else this._noteSyncError('network', (e && e.message) || 'Stream connection failed.');
            // Take over reconnection so we can apply exponential backoff instead
            // of the library's fixed retry cadence.
            this._scheduleReconnect();
        });
    }

    /** Records that the current stream is alive, and pushes its deadline out. */
    _touch() {
        const now = Date.now();
        if (this._idleTimeoutMs > 0) this._watchdogDeadline = now + this._idleTimeoutMs;
        // A live, bootstrapped stream with no news means the flags are current.
        if (this._state === 'streaming' && !this._staged) this._lastSyncAt = now;
    }

    /** Declares the current stream dead `ms` from now unless _touch() moves that. */
    _armWatchdog(ms) {
        const deadline = Date.now() + ms;
        this._watchdogDeadline = deadline;
        if (this._watchdog && this._watchdogFireAt <= deadline) return;
        if (this._watchdog) clearTimeout(this._watchdog);
        this._scheduleWatchdog(ms);
    }

    _scheduleWatchdog(ms) {
        this._watchdogFireAt = Date.now() + ms;
        this._watchdog = setTimeout(() => {
            this._watchdog = null;
            if (!this.sse || this._watchdogDeadline === 0) return;
            const left = this._watchdogDeadline - Date.now();
            if (left > 0) {
                this._scheduleWatchdog(left);
                return;
            }
            console.warn('[Checkgate] Stream silent past its deadline — treating it as dead and reconnecting.');
            this._noteSyncError('timeout', 'Stream silent past its deadline.');
            this._scheduleReconnect();
        }, ms);
        if (this._watchdog.unref) this._watchdog.unref();
    }

    _disarmWatchdog() {
        this._watchdogDeadline = 0;
        if (this._watchdog) {
            clearTimeout(this._watchdog);
            this._watchdog = null;
        }
    }

    _closeStream() {
        this._disarmWatchdog();
        const es = this.sse;
        this.sse = null;
        if (es) es.close();
    }

    /**
     * The server rejected the SDK key: stop every retry (stream and poll),
     * keep serving the flags already loaded, and settle connect().
     */
    _failAuth(status) {
        const err = new CheckgateAuthError(status);
        console.error(err.message);
        this._noteSyncError('auth', `SDK key rejected (HTTP ${status}).`);
        this._closeStream();
        if (this._reconnectTimer) {
            clearTimeout(this._reconnectTimer);
            this._reconnectTimer = null;
        }
        this._stopPollFallback();
        this._staged = null;
        this._stagedRevisions = null;
        this._reconnectAttempts = 0;
        // The error first: status listeners read it from getStatus().
        this._lastError = err;
        this._setState('auth_failed');
        const reject = this._rejectReady;
        this._resolveReady = null;
        this._rejectReady = null;
        this._connectPromise = null;
        if (reject) reject(err);
    }

    /**
     * Applies one stream change event. While a (re)connect replay is in progress
     * the event is staged for "ready"; afterwards it is applied unless it is
     * older than the revision the client already holds for that flag.
     */
    _applyDelta(event) {
        const isUpsert = event && event.type === 'UPSERT';
        if (!isUpsert && !(event && event.type === 'DELETE')) return;
        const flag = isUpsert ? event.flag : null;
        const key = isUpsert ? flag && flag.key : event.key;
        if (typeof key !== 'string') return;
        const revision = typeof event.revision === 'number' ? event.revision : null;

        this._noteRevision(revision);
        if (this._staged) {
            if (isStaleEvent(this._stagedRevisions.get(key), revision, isUpsert)) {
                this._staleEventsIgnored++;
                return;
            }
            if (isUpsert) this._staged.set(key, flag);
            else this._staged.delete(key);
            if (revision !== null) this._stagedRevisions.set(key, { revision, deleted: !isUpsert });
            return;
        }

        if (isStaleEvent(this._revisions.get(key), revision, isUpsert)) {
            this._staleEventsIgnored++;
            return;
        }
        if (revision !== null) this._revisions.set(key, { revision, deleted: !isUpsert, at: ++this._learnSeq });
        if (isUpsert) {
            // Pass the full flag (flag_type, default/disabled values, and per-rule
            // variants) so getValue()/getVariant() resolve non-boolean flags.
            this.core.upsertFlagV2(JSON.stringify(flag));
            this._flagCache.set(key, flag);
        } else {
            // A delete of a flag the client does not hold changes nothing.
            if (!this._flagCache.delete(key)) return;
            this.core.deleteFlag(key);
        }
        this._emitChange(key);
        this._persistCache(); // keep the offline snapshot fresh
    }

    /**
     * Closes the current stream and schedules a reconnect using exponential
     * backoff with jitter, capped at maxReconnectDelayMs. No-op if the client
     * was closed by the caller or a reconnect is already pending.
     */
    _scheduleReconnect() {
        if (this._userClosed || this._reconnectTimer || this._state === 'auth_failed') return;
        this._closeStream();

        const base = this.reconnectDelayMs * Math.pow(2, this._reconnectAttempts);
        const capped = Math.min(this.maxReconnectDelayMs, base);
        const delay = capped + Math.random() * capped * 0.3; // up to 30% jitter
        this._reconnectAttempts++;
        this._reconnectCount++;
        // A replay cut short never replaces the live snapshot.
        this._staged = null;
        this._stagedRevisions = null;

        // SSE is failing repeatedly (e.g. a proxy blocking long-lived connections) —
        // fall back to polling so flags still stay current. SSE retries continue in
        // the background; a successful reconnect stops the poll loop again.
        if (this._reconnectAttempts >= this.pollFallbackThreshold) {
            this._startPollFallback();
        }
        this._setState(this._polling ? 'polling' : (this._ready ? 'reconnecting' : 'connecting'));

        console.warn(`[Checkgate] Stream disconnected — reconnecting in ${Math.round(delay)}ms.`);
        this._reconnectTimer = setTimeout(() => {
            this._reconnectTimer = null;
            if (!this._userClosed) this._connectDeltas();
        }, delay);
        if (this._reconnectTimer.unref) this._reconnectTimer.unref();
    }

    // ---------------------------------------------------------------------
    // Poll fallback (used when SSE cannot be established at all)
    // ---------------------------------------------------------------------

    /** Whether the client is currently polling instead of streaming. */
    isPolling() {
        return this._polling;
    }

    _startPollFallback() {
        if (this._polling) return;
        this._polling = true;
        console.warn(
            `[Checkgate] SSE reconnect failing repeatedly — falling back to polling ` +
            `${this.serverUrl}/flags/snapshot every ${this.pollIntervalMs}ms.`
        );
        this._pollSnapshot(); // poll immediately rather than waiting for the first interval tick
        this._pollTimer = setInterval(() => this._pollSnapshot(), this.pollIntervalMs);
        if (this._pollTimer.unref) this._pollTimer.unref();
    }

    _stopPollFallback() {
        this._syncGeneration++;
        if (this._pollAbort) this._pollAbort.abort();
        this._pollAbort = null;
        this._pollPromise = null;
        this._polling = false;
        if (this._pollTimer) {
            clearInterval(this._pollTimer);
            this._pollTimer = null;
        }
    }

    /**
     * Fetches the full flag snapshot and applies it, diffing against the last
     * known state (`_flagCache`) so only genuinely changed flags trigger
     * onChange listeners. Resolves connect() if this is the first successful
     * sync (SSE may never have connected at all). Best-effort: network/parse
     * failures are logged and retried on the next interval tick.
     */
    _pollSnapshot() {
        if (this._userClosed) return Promise.resolve();
        if (this._pollPromise) return this._pollPromise;
        const generation = this._syncGeneration;
        const controller = typeof AbortController === 'function' ? new AbortController() : null;
        this._pollAbort = controller;
        const pending = this._fetchSnapshot(generation, controller).finally(() => {
            if (this._pollPromise === pending) {
                this._pollPromise = null;
                this._pollAbort = null;
            }
        });
        this._pollPromise = pending;
        return pending;
    }

    async _fetchSnapshot(generation, controller) {
        // The server reads the snapshot after this point (see mergeFullSet).
        const startedAt = this._learnSeq;
        try {
            // `v=2` asks for per-flag revisions and recent deletions; servers
            // before 0.1.31 ignore it and send a bare array.
            const res = await fetch(`${this.serverUrl}/flags/snapshot?v=2`, {
                ...(controller ? { signal: controller.signal } : {}),
                headers: this.sdkKey ? { 'Authorization': `Bearer ${this.sdkKey}` } : {},
            });
            if (!res.ok) {
                if (generation !== this._syncGeneration || this._userClosed) return;
                if (AUTH_FAILURE_STATUSES.has(res.status)) {
                    this._failAuth(res.status);
                    return;
                }
                console.warn(`[Checkgate] Poll fallback request failed: HTTP ${res.status}`);
                this._pollFailed('http', `Poll request failed: HTTP ${res.status}`);
                return;
            }
            const body = await res.json();
            if (generation !== this._syncGeneration || this._userClosed) return;
            let snapshot;
            try {
                snapshot = parseSnapshot(body);
            } catch (err) {
                console.warn('[Checkgate] Poll fallback request failed:', err.message);
                this._pollFailed('parse', err.message);
                return;
            }
            const { flags, revisions } = snapshot;
            const envId = res.headers?.get('X-Checkgate-Environment-Id');
            if (envId) this._envId = envId;
            this._applySnapshot(flags, revisions, startedAt);
            this._hydrated = true;
            this._source = 'server';
            this._lastPollAt = Date.now();
            this._startImpressionTimer();
            this._markReady();
        } catch (err) {
            if (generation === this._syncGeneration && !this._userClosed) {
                console.warn('[Checkgate] Poll fallback request failed:', err && err.message);
                // A body that is not JSON surfaces here as a SyntaxError.
                this._pollFailed(err && err.name === 'SyntaxError' ? 'parse' : 'network',
                    (err && err.message) || 'Poll request failed.');
            }
        }
    }

    /**
     * Applies a complete flag set (a stream replay as a Map, or a poll snapshot's array),
     * merged with what the client holds by `mergeFullSet`: `revisions`
     * (flag key → { revision, deleted }) is null when the server sent none,
     * and `startedAt` is the learn counter when the set was requested. Then
     * diffs against `_flagCache` so only flags that actually changed (added,
     * updated, or removed) trigger onChange — after the whole set is applied —
     * and a cache write. Returns true if anything changed.
     */
    _applySnapshot(flags, revisions = null, startedAt = this._learnSeq) {
        // A replay arrives as the staged Map (key → flag); a poll as an array.
        let incoming = flags;
        if (!(flags instanceof Map)) {
            incoming = new Map();
            for (const flag of flags) incoming.set(flag.key, flag);
        }
        if (revisions) for (const entry of revisions.values()) this._noteRevision(entry.revision);
        const merged = mergeFullSet(
            this._flagCache, this._revisions, incoming, revisions, startedAt, ++this._learnSeq);
        this._staleEventsIgnored += merged.ignored;
        const newKeys = new Set();
        const changed = [];

        for (const flag of merged.flags.values()) {
            newKeys.add(flag.key);
            const prev = this._flagCache.get(flag.key);
            if (prev && JSON.stringify(prev) === JSON.stringify(flag)) continue;

            this.core.upsertFlagV2(JSON.stringify(flag));
            this._flagCache.set(flag.key, flag);
            changed.push(flag.key);
        }

        for (const key of Array.from(this._flagCache.keys())) {
            if (newKeys.has(key)) continue;
            this.core.deleteFlag(key);
            this._flagCache.delete(key);
            changed.push(key);
        }

        this._revisions = merged.revisions;
        if (changed.length === 0) return false;
        this._persistCache();
        // Notify only once the whole snapshot is in place: a listener that
        // evaluates a flag must not see some of its prerequisites updated and
        // others not yet.
        if (this._ready) changed.forEach((key) => this._emitChange(key));
        return true;
    }

    /**
     * Evaluates a feature flag synchronously, with no network IO (a few microseconds;
     * see docs/performance/benchmarks.md).
     *
     * @param {string} flagKey
     * @param {string} userKey - Stable user identifier used for rollout hashing
     * @param {Record<string, string>} [userAttributes={}] - User attributes for targeting rules
     * @returns {boolean}
     */
    isEnabled(flagKey, userKey, userAttributes = {}) {
        if (!this._ready && !this._hydrated) {
            console.warn('[Checkgate] isEnabled() called before connect() resolved. Returning false.');
            return false;
        }
        const result = this.core.isEnabled(flagKey, userKey, userAttributes);
        this._recordImpression(flagKey, userKey, result ? 'true' : 'false', userAttributes);
        return result;
    }

    /**
     * Evaluates a multi-variant flag and returns the full result
     * `{ enabled, value }`, where `value` is the resolved variant
     * (boolean, string, number, or JSON object).
     *
     * @param {string} flagKey
     * @param {string} userKey
     * @param {Record<string, string>} [userAttributes={}]
     * @returns {{ enabled: boolean, value: boolean|string|number|object|null } | null}
     *          `null` if the flag does not exist.
     */
    getVariant(flagKey, userKey, userAttributes = {}) {
        if (!this._ready && !this._hydrated) {
            console.warn('[Checkgate] getVariant() called before connect() resolved. Returning null.');
            return null;
        }
        let variant;
        try {
            variant = JSON.parse(this.core.getVariant(flagKey, userKey, userAttributes));
        } catch {
            return null;
        }
        if (variant !== null) {
            this._recordImpression(
                flagKey, userKey, this._formatValue(variant.value), userAttributes);
        }
        return variant;
    }

    /**
     * Evaluates a multi-variant flag and returns just its resolved value.
     *
     * @param {string} flagKey
     * @param {string} userKey
     * @param {Record<string, string>} [userAttributes={}]
     * @param {*} [defaultValue=null] - returned when the flag does not exist.
     * @returns {boolean|string|number|object|null}
     */
    getValue(flagKey, userKey, userAttributes = {}, defaultValue = null) {
        const variant = this.getVariant(flagKey, userKey, userAttributes);
        return variant === null ? defaultValue : variant.value;
    }

    /**
     * Evaluates a flag and explains the result: `{ enabled, value, reason }`
     * plus `ruleIndex`, `rolloutBucket`, `variantIndex` and `prerequisites`
     * when they apply. Same evaluation as getVariant() (same engine code),
     * locally and without network IO; slightly more work, since it records
     * the path. Reasons: `DISABLED`, `PREREQUISITE_FAILED`, `TARGETING_MATCH`,
     * `ROLLOUT_EXCLUDED`, `SPLIT`, `DEFAULT`, `STATIC`.
     *
     * @param {string} flagKey
     * @param {string} userKey
     * @param {Record<string, string>} [userAttributes={}]
     * @returns {object|null} `null` if the flag does not exist (or before
     *          connect() resolved and nothing was hydrated).
     */
    getDetails(flagKey, userKey, userAttributes = {}) {
        if (!this._ready && !this._hydrated) {
            console.warn('[Checkgate] getDetails() called before connect() resolved. Returning null.');
            return null;
        }
        let raw;
        try {
            raw = JSON.parse(this.core.explain(flagKey, userKey, userAttributes));
        } catch {
            return null;
        }
        if (raw === null) return null;
        this._recordImpression(flagKey, userKey, this._formatValue(raw.value), userAttributes);
        return toDetails(raw);
    }

    // ---------------------------------------------------------------------
    // Impression reporting (async, batched, best-effort)
    // ---------------------------------------------------------------------

    /** Serialize a resolved variant value into the string stored server-side. */
    _formatValue(value) {
        if (value === null || value === undefined) return 'null';
        if (typeof value === 'object') return JSON.stringify(value);
        return String(value);
    }

    /** Buffer one evaluation event; flush when the batch fills up. */
    _recordImpression(flagKey, userKey, value, attributes) {
        if (!this.reportImpressions || !this._envId) return;

        this._impressions.push({
            flag_key: flagKey,
            user_id: userKey,
            value,
            // When the evaluation happened, not when the batch reaches the
            // server: impressions and conversions flush independently, and
            // experiment attribution needs the real order.
            evaluated_at: new Date().toISOString(),
            // Attributes are omitted unless explicitly opted in — the whole point
            // of local evaluation is that user attributes never leave the process.
            ...(this.sendEvaluationContext ? { context: attributes } : {}),
        });

        // Bound memory if the server is unreachable: keep only the newest events.
        if (this._impressions.length > 10000) {
            this._impressions.splice(0, this._impressions.length - 10000);
        }
        if (this._impressions.length >= this.impressionBatchSize) {
            this._flushImpressions();
        }
    }

    _startImpressionTimer() {
        // Not gated on reportImpressions: the timer also flushes track() events,
        // and both flush methods no-op on empty buffers, so an idle timer is cheap.
        if (this._flushTimer) return;
        this._flushTimer = setInterval(() => {
            this._flushImpressions();
            this._flushEvents();
        }, this.impressionFlushIntervalMs);
        // Don't keep the Node event loop alive just to flush analytics.
        if (this._flushTimer.unref) this._flushTimer.unref();
    }

    /** POST buffered impressions (up to the server's 500/batch limit). */
    _flushImpressions() {
        if (!this._envId || this._impressions.length === 0) return;
        const batch = this._impressions.splice(0, 500);

        fetch(`${this.serverUrl}/api/environments/${this._envId}/impressions`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                ...(this.sdkKey ? { 'Authorization': `Bearer ${this.sdkKey}` } : {}),
            },
            body: JSON.stringify(batch),
        }).catch((err) => {
            console.warn('[Checkgate] Failed to report impressions:', err && err.message);
        });
    }

    // ---------------------------------------------------------------------
    // Goal event tracking (A/B testing conversions)
    // ---------------------------------------------------------------------

    /**
     * Records a goal/conversion event for A/B testing (e.g. "checkout_complete").
     * Buffered and reported asynchronously, mirroring impression reporting.
     *
     * @param {string} eventKey - The goal event name (must match the experiment's goal).
     * @param {string} userKey - The same user identifier passed to getVariant()/isEnabled().
     * @param {object} [opts]
     * @param {number} [opts.value] - Optional numeric payload (e.g. revenue).
     * @param {object} [opts.context] - Optional metadata stored with the event.
     */
    track(eventKey, userKey, opts = {}) {
        if (!eventKey || typeof eventKey !== 'string') {
            console.warn('[Checkgate] track() called without a valid eventKey.');
            return;
        }
        if (!this._envId) {
            console.warn('[Checkgate] track() called before connect() resolved — event dropped.');
            return;
        }

        this._events.push({
            event_key: eventKey,
            user_id: userKey,
            occurred_at: new Date().toISOString(),
            ...(opts.value != null ? { value: opts.value } : {}),
            ...(opts.context != null ? { context: opts.context } : {}),
        });

        // Bound memory if the server is unreachable: keep only the newest events.
        if (this._events.length > 10000) {
            this._events.splice(0, this._events.length - 10000);
        }
        // Ensure events flush even when impression auto-reporting is disabled.
        this._startImpressionTimer();
        if (this._events.length >= this.impressionBatchSize) {
            this._flushEvents();
        }
    }

    /** POST buffered goal events (up to the server's 500/batch limit). */
    _flushEvents() {
        if (!this._envId || this._events.length === 0) return;
        const batch = this._events.splice(0, 500);

        fetch(`${this.serverUrl}/api/environments/${this._envId}/events`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                ...(this.sdkKey ? { 'Authorization': `Bearer ${this.sdkKey}` } : {}),
            },
            body: JSON.stringify(batch),
        }).catch((err) => {
            console.warn('[Checkgate] Failed to report events:', err && err.message);
        });
    }

    // ---------------------------------------------------------------------
    // Offline persistence (hydrate on start, persist on bootstrap/delta)
    // ---------------------------------------------------------------------

    /**
     * Loads the persisted flag snapshot into the evaluation core so flags can be
     * evaluated offline before the first successful connection. Best-effort.
     */
    async _hydrateFromCache() {
        if (!this.storage) return;
        try {
            const raw = await this.storage.getItem(this._cacheKey);
            if (!raw) return;
            const snapshot = JSON.parse(raw);
            for (const key of Object.keys(snapshot)) {
                const flag = snapshot[key];
                this._flagCache.set(key, flag);
                this.core.upsertFlagV2(JSON.stringify(flag));
            }
            // Only mark hydrated once the live stream hasn't already taken over.
            if (!this._ready) {
                this._hydrated = true;
                this._hydratedFromCache = true;
                if (this._source === 'none') this._source = 'offline_cache';
            }
        } catch (err) {
            console.warn('[Checkgate] Failed to hydrate flag cache:', err && err.message);
        }
    }

    /** Persists the current flag set to `storage` (best-effort, fire-and-forget). */
    _persistCache() {
        if (!this.storage) return;
        try {
            const snapshot = Object.fromEntries(this._flagCache);
            Promise.resolve(this.storage.setItem(this._cacheKey, JSON.stringify(snapshot)))
                .then(() => { this._lastPersistAt = Date.now(); })
                .catch((err) => console.warn('[Checkgate] Failed to persist flag cache:', err && err.message));
        } catch (err) {
            console.warn('[Checkgate] Failed to persist flag cache:', err && err.message);
        }
    }

    /**
     * Closes the SSE connection and cleans up resources.
     */
    disconnect() {
        // Stop managed reconnection before tearing down the stream.
        this._userClosed = true;
        this._setState('closed');
        if (this._reconnectTimer) {
            clearTimeout(this._reconnectTimer);
            this._reconnectTimer = null;
        }
        this._stopPollFallback();
        this._flushImpressions();
        this._flushEvents();
        if (this._flushTimer) {
            clearInterval(this._flushTimer);
            this._flushTimer = null;
        }
        this._closeStream();
        // Reset readiness so a later connect() re-establishes the stream.
        this._ready = false;
        this._staged = null;
        this._stagedRevisions = null;
        // A connect() still waiting would otherwise never settle.
        const reject = this._rejectReady;
        this._connectPromise = null;
        this._resolveReady = null;
        this._rejectReady = null;
        this._reconnectAttempts = 0;
        if (reject) reject(new Error('[Checkgate] disconnect() was called before connect() completed.'));
    }
}

module.exports = { CheckgateClient, CheckgateAuthError };
