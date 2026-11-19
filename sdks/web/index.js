import { cacheNamespace } from './cache-key.js';
import init, { CheckgateCoreWasm } from './dist/checkgate.js';

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
 * Returns `{ flags, revisions }`; revision entries carry `at`, the learn
 * counter value when the client learned them.
 */
function mergeFullSet(flags, revisions, incoming, incomingRevisions, startedAt, at) {
    if (!incomingRevisions) return { flags: new Map(incoming), revisions: new Map() };
    const outFlags = new Map();
    const outRevisions = new Map();
    const visit = (key) => {
        const held = revisions.get(key);
        const offered = incomingRevisions.get(key);
        if (held && held.at > startedAt && (!offered || !supersedes(offered, held))) {
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
    return { flags: outFlags, revisions: outRevisions };
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
export class CheckgateAuthError extends Error {
    constructor(status) {
        super(`[Checkgate] The server rejected the SDK key (HTTP ${status}). ` +
            'Not retrying; check the key, then call connect() again.');
        this.name = 'CheckgateAuthError';
        this.status = status;
    }
}

/**
 * The HTTP status carried by a stream error event, if it carries one. Browser
 * `EventSource` never exposes it (see _checkCredentials); polyfills may.
 */
function errorStatus(event) {
    if (!event) return undefined;
    const code = typeof event.code === 'number' ? event.code : event.xhrStatus;
    return typeof code === 'number' && code > 0 ? code : undefined;
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

export class CheckgateWeb {
    /**
     * @param {Object} options
     * @param {string} options.serverUrl - Base URL of your Checkgate server
     * @param {string} [options.sdkKey] - SDK key (stream: `?sdk_key=` query parameter,
     *        since EventSource cannot set headers; polls: Authorization: Bearer)
     * @param {number} [options.reconnectDelayMs=3000] - Reconnect delay on SSE disconnect
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
     *        reported impressions. Off by default so attributes never leave the browser.
     * @param {Array<object>|{flags: Array<object>}} [options.bootstrap] - A flag snapshot to
     *        load synchronously on connect, before the SSE stream is established — pass the
     *        `flags` from a server-rendered `@checkgate/ssr` bootstrap payload so the client
     *        evaluates correctly on first paint with zero flag flicker.
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
        storage = (typeof localStorage !== 'undefined' ? localStorage : null),
        bootstrap = null,
    } = {}) {
        this.serverUrl = serverUrl;
        this.sdkKey = sdkKey;
        this.reconnectDelayMs = reconnectDelayMs;
        this.maxReconnectDelayMs = maxReconnectDelayMs;
        this.core = null;
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
        this.sse = null;

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
        this._credentialCheck = null;

        // Offline persistence: shadow copy of the raw flag set, persisted to
        // `storage` (defaults to localStorage) and re-hydrated on the next load
        // so flags can be evaluated before (or without) a live connection.
        this.storage = storage;
        this._flagCache = new Map();
        this._hydrated = false;
        this._cacheKey = `checkgate:flags:v2:${serverUrl}:${cacheNamespace(sdkKey || 'session')}`;

        // SSR bootstrap: a flag snapshot to seed the core on connect so the very
        // first evaluation on the client matches what the server rendered.
        // Accepts a raw flags array or a full @checkgate/ssr payload ({ flags }).
        this._bootstrapFlags = Array.isArray(bootstrap)
            ? bootstrap
            : (bootstrap && Array.isArray(bootstrap.flags) ? bootstrap.flags : null);

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
     * Initializes the WebAssembly module and opens the SSE stream.
     * Resolves once the server has replayed the full flag set and sent the
     * "ready" event. The server sends the full flag state on connect, so no
     * separate REST bootstrap fetch is needed.
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
        this._state = this._ready ? 'reconnecting' : 'connecting';
        const pending = new Promise((resolve, reject) => {
            this._resolveReady = resolve;
            this._rejectReady = reject;
        });
        // A caller that does not await connect() must not get an unhandled
        // rejection; one that awaits still sees it.
        pending.catch(() => {});
        this._connectPromise = pending;
        (async () => {
            await init();
            // One core per client: `_flagCache` survives disconnect(), and a
            // fresh core would miss every flag the next replay finds unchanged.
            if (!this.core) this.core = new CheckgateCoreWasm();
            if (this._bootstrapFlags && this._bootstrapFlags.length > 0) {
                // Prefer the SSR bootstrap (freshest — rendered for this request)
                // over any stale persisted snapshot.
                for (const flag of this._bootstrapFlags) {
                    this._flagCache.set(flag.key, flag);
                    this.core.upsert_flag_v2(JSON.stringify(flag));
                }
                this._hydrated = true;
            } else {
                // Hydrate from the persisted snapshot so evaluations work offline
                // immediately, then open the live stream.
                await this._hydrateFromCache();
            }
            // disconnect() during initialisation has already settled this
            // connect(); opening a stream now would leak it.
            if (this._userClosed || this._connectPromise !== pending) return;
            this._connectDeltas();
        })().catch((err) => {
            if (this._connectPromise !== pending) return;
            const reject = this._rejectReady;
            this._connectPromise = null;
            this._resolveReady = null;
            this._rejectReady = null;
            if (reject) reject(err);
        });
        return pending;
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

    _markReady() {
        this._lastSyncAt = Date.now();
        this._ready = true;
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
     * Loads the persisted flag snapshot into the Wasm core so flags can be
     * evaluated offline before the first successful connection. Best-effort.
     */
    async _hydrateFromCache() {
        if (!this.storage || !this.core) return;
        try {
            const raw = await this.storage.getItem(this._cacheKey);
            if (!raw) return;
            const snapshot = JSON.parse(raw);
            for (const key of Object.keys(snapshot)) {
                const flag = snapshot[key];
                this._flagCache.set(key, flag);
                this.core.upsert_flag_v2(JSON.stringify(flag));
            }
            if (!this._ready) this._hydrated = true;
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
                .catch((err) => console.warn('[Checkgate] Failed to persist flag cache:', err && err.message));
        } catch (err) {
            console.warn('[Checkgate] Failed to persist flag cache:', err && err.message);
        }
    }

    _connectDeltas() {
        this._closeStream();
        // The replay this stream sends is read after this point.
        this._streamStartedAt = this._learnSeq;

        // Browser EventSource doesn't support custom headers; use sdkKey as a
        // query param when auth is required.
        const url = this.sdkKey
            ? `${this.serverUrl}/stream?sdk_key=${encodeURIComponent(this.sdkKey)}`
            : `${this.serverUrl}/stream`;

        const es = new EventSource(url);
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
            // Persist the freshly-bootstrapped full flag set for offline use.
            this._persistCache();
            this._state = 'streaming';
            this._markReady();
        });

        on('update', (e) => {
            let event;
            try {
                event = JSON.parse(e.data);
            } catch (err) {
                console.error('[Checkgate] Failed to parse delta update:', err);
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
            // Take over reconnection so we can apply exponential backoff instead
            // of the browser's fixed retry cadence.
            this._scheduleReconnect();
            // A browser does not say why a connection attempt failed. If it
            // never got as far as "connected", ask the snapshot endpoint, which
            // takes the same key: a 401/403 there ends the retries.
            if (!connected && status === undefined) this._checkCredentials();
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
            this._scheduleReconnect();
        }, ms);
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
     * Asks `GET /flags/snapshot` whether the SDK key is still accepted, after a
     * stream attempt failed for a reason the browser does not reveal. At most
     * one check is in flight; it runs once per failed attempt, so it is bounded
     * by the reconnect backoff. Only a 401/403 changes anything.
     */
    _checkCredentials() {
        if (this._credentialCheck || typeof fetch !== 'function') return;
        const generation = this._syncGeneration;
        this._credentialCheck = fetch(`${this.serverUrl}/flags/snapshot`, {
            headers: this.sdkKey ? { 'Authorization': `Bearer ${this.sdkKey}` } : {},
        }).then((res) => {
            if (this._userClosed || generation !== this._syncGeneration) return;
            if (AUTH_FAILURE_STATUSES.has(res.status)) this._failAuth(res.status);
        }, () => {
            // Unreachable: the reconnect already scheduled handles it.
        }).finally(() => {
            this._credentialCheck = null;
        });
    }

    /**
     * The server rejected the SDK key: stop every retry (stream and poll),
     * keep serving the flags already loaded, and settle connect().
     */
    _failAuth(status) {
        const err = new CheckgateAuthError(status);
        console.error(err.message);
        this._closeStream();
        if (this._reconnectTimer) {
            clearTimeout(this._reconnectTimer);
            this._reconnectTimer = null;
        }
        this._stopPollFallback();
        this._staged = null;
        this._stagedRevisions = null;
        this._reconnectAttempts = 0;
        this._state = 'auth_failed';
        this._lastError = err;
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

        if (this._staged) {
            if (isStaleEvent(this._stagedRevisions.get(key), revision, isUpsert)) return;
            if (isUpsert) this._staged.set(key, flag);
            else this._staged.delete(key);
            if (revision !== null) this._stagedRevisions.set(key, { revision, deleted: !isUpsert });
            return;
        }

        if (isStaleEvent(this._revisions.get(key), revision, isUpsert)) return;
        if (revision !== null) this._revisions.set(key, { revision, deleted: !isUpsert, at: ++this._learnSeq });
        if (isUpsert) {
            // Pass the full flag (flag_type, default/disabled values, and per-rule
            // variants) so getValue()/getVariant() resolve non-boolean flags.
            this.core.upsert_flag_v2(JSON.stringify(flag));
            this._flagCache.set(key, flag);
        } else {
            // A delete of a flag the client does not hold changes nothing.
            if (!this._flagCache.delete(key)) return;
            this.core.delete_flag(key);
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
        // A replay cut short never replaces the live snapshot.
        this._staged = null;
        this._stagedRevisions = null;

        // SSE is failing repeatedly (e.g. a proxy blocking long-lived connections) —
        // fall back to polling so flags still stay current. SSE retries continue in
        // the background; a successful reconnect stops the poll loop again.
        if (this._reconnectAttempts >= this.pollFallbackThreshold) {
            this._startPollFallback();
        }
        this._state = this._polling ? 'polling' : (this._ready ? 'reconnecting' : 'connecting');

        console.warn(`[Checkgate] Stream disconnected — reconnecting in ${Math.round(delay)}ms.`);
        this._reconnectTimer = setTimeout(() => {
            this._reconnectTimer = null;
            if (!this._userClosed) this._connectDeltas();
        }, delay);
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
            `[Checkgate] Stream failing repeatedly — falling back to polling ` +
            `${this.serverUrl}/flags/snapshot every ${this.pollIntervalMs}ms.`
        );
        this._pollSnapshot(); // poll immediately rather than waiting for the first interval tick
        this._pollTimer = setInterval(() => this._pollSnapshot(), this.pollIntervalMs);
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
     *
     * Uses a Bearer header (fetch supports custom headers, unlike EventSource),
     * avoiding the `?sdk_key=` query-param leakage the SSE connection needs.
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
        if (!this.core) return;
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
                return;
            }
            const body = await res.json();
            if (generation !== this._syncGeneration || this._userClosed) return;
            const { flags, revisions } = parseSnapshot(body);
            const envId = res.headers?.get('X-Checkgate-Environment-Id');
            if (envId) this._envId = envId;
            this._applySnapshot(flags, revisions, startedAt);
            this._hydrated = true;
            this._startImpressionTimer();
            if (!this.sse) this._state = 'polling';
            this._markReady();
        } catch (err) {
            if (generation === this._syncGeneration && !this._userClosed) {
                console.warn('[Checkgate] Poll fallback request failed:', err && err.message);
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
        const merged = mergeFullSet(
            this._flagCache, this._revisions, incoming, revisions, startedAt, ++this._learnSeq);
        const newKeys = new Set();
        const changed = [];

        for (const flag of merged.flags.values()) {
            newKeys.add(flag.key);
            const prev = this._flagCache.get(flag.key);
            if (prev && JSON.stringify(prev) === JSON.stringify(flag)) continue;

            this.core.upsert_flag_v2(JSON.stringify(flag));
            this._flagCache.set(flag.key, flag);
            changed.push(flag.key);
        }

        for (const key of Array.from(this._flagCache.keys())) {
            if (newKeys.has(key)) continue;
            this.core.delete_flag(key);
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
     * Evaluate a flag locally in the browser via WebAssembly (no network call).
     *
     * @param {string} flagKey
     * @param {string} userKey - Stable user identifier
     * @param {Record<string, string>} [userAttributes={}]
     * @returns {boolean}
     */
    isEnabled(flagKey, userKey, userAttributes = {}) {
        if ((!this._ready && !this._hydrated) || !this.core) {
            console.warn('[Checkgate] isEnabled() called before connect() resolved. Returning false.');
            return false;
        }
        const result = this.core.is_enabled(flagKey, userKey, userAttributes);
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
        if ((!this._ready && !this._hydrated) || !this.core) {
            console.warn('[Checkgate] getVariant() called before connect() resolved. Returning null.');
            return null;
        }
        // The Wasm binding returns a plain JS object, or null if not found.
        const result = this.core.get_variant(flagKey, userKey, userAttributes);
        const variant = result ?? null;
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
            // of local evaluation is that user attributes never leave the browser.
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
            keepalive: true,
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
            keepalive: true,
        }).catch((err) => {
            console.warn('[Checkgate] Failed to report events:', err && err.message);
        });
    }

    /**
     * Closes the SSE connection.
     */
    disconnect() {
        // Stop managed reconnection before tearing down the stream.
        this._userClosed = true;
        this._state = 'closed';
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
