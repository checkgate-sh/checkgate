import { cacheNamespace } from './cache-key.js';
/**
 * Checkgate React Native SDK
 *
 * Bridges the JS layer to the Rust evaluation engine via the C++ JSI host
 * installed as `global.__CheckgateInternal` by `installCheckgateJSI()`.
 *
 * The server sends the full flag state on every SSE (re)connect, so:
 *   1. Open SSE stream.
 *   2. On "connected" → clear the Rust cache.
 *   3. Incoming UPSERT events rebuild the cache.
 *   4. isEnabled() evaluates synchronously — zero network IO, sub-microsecond.
 */

export class CheckgateNativeClient {
    /**
     * @param {Object} options
     * @param {string} options.serverUrl - Base URL of your Checkgate server
     * @param {string} [options.sdkKey] - SDK key for authentication
     * @param {number} [options.reconnectDelayMs=5000] - SSE reconnect delay in ms
     * @param {number} [options.maxReconnectDelayMs=30000] - Maximum backoff delay between reconnects
     * @param {number} [options.pollFallbackThreshold=3] - Consecutive failed SSE reconnects
     *        before falling back to polling GET /flags/snapshot. Set to Infinity to disable.
     * @param {number} [options.pollIntervalMs=30000] - Poll interval while in fallback mode.
     * @param {boolean} [options.reportImpressions=true] - Report evaluation events for analytics
     * @param {number} [options.impressionBatchSize=50] - Flush after this many buffered events
     * @param {number} [options.impressionFlushIntervalMs=10000] - Periodic flush interval
     * @param {boolean} [options.sendEvaluationContext=false] - Include user attributes in
     *        reported impressions. Off by default so attributes never leave the device.
     */
    constructor({
        serverUrl,
        sdkKey,
        reconnectDelayMs = 5000,
        maxReconnectDelayMs = 30000,
        pollFallbackThreshold = 3,
        pollIntervalMs = 30000,
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
        this._ready = false;
        this._bootstrapping = false;
        this._connectPromise = null;
        this._resolveReady = null;
        this._changeListeners = [];
        this.sse = null;
        // The internal module exposed by the C++ JSI installation
        this.bridge = global.__CheckgateInternal;

        // Offline persistence: shadow copy of the raw flag set, persisted to
        // `storage` (e.g. an AsyncStorage adapter) and re-hydrated on the next
        // cold start so flags evaluate before (or without) a live connection.
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
     * Opens the SSE connection and starts streaming flags.
     * Resolves once the server has replayed the full flag set and sent the
     * "ready" event — consistent with the Node and Web SDKs. Calling connect()
     * again while a connection is in progress returns the same pending promise.
     *
     * @returns {Promise<void>}
     */
    connect() {
        if (this._ready) return Promise.resolve();
        if (this._connectPromise) return this._connectPromise;

        if (!this.bridge) {
            return Promise.reject(new Error(
                '[Checkgate] JSI module not found. Ensure installCheckgateJSI() was called ' +
                'in your native module before JS starts.'
            ));
        }

        this._userClosed = false;
        this._connectPromise = new Promise((resolve) => {
            this._resolveReady = resolve;
        });
        // Hydrate from the persisted snapshot so evaluations work offline
        // immediately, then open the live stream.
        this._hydrateFromCache().finally(() => {
            if (!this._userClosed) this._connectDeltas();
        });
        return this._connectPromise;
    }

    /**
     * Whether the initial bootstrap has completed and flags are ready to evaluate.
     * @returns {boolean}
     */
    isReady() {
        return this._ready;
    }

    /**
     * Registers a listener invoked whenever a flag changes after the initial
     * bootstrap (created, updated, or deleted). The flag's key is passed to the
     * callback. Bootstrap and reconnect re-syncs do not trigger listeners.
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
     * Loads the persisted flag snapshot into the native store so flags can be
     * evaluated offline before the first successful connection. Best-effort.
     */
    async _hydrateFromCache() {
        if (!this.storage || !this.bridge) return;
        try {
            const raw = await this.storage.getItem(this._cacheKey);
            if (!raw) return;
            const snapshot = JSON.parse(raw);
            for (const key of Object.keys(snapshot)) {
                const flag = snapshot[key];
                this._flagCache.set(key, flag);
                this.bridge.upsertFlagV2(flag);
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
        if (this.sse) {
            this.sse.close();
        }

        // React Native's built-in fetch-based EventSource (or the `event-source`
        // package) supports headers, so we send auth via Authorization header.
        this.sse = new EventSource(`${this.serverUrl}/stream`, {
            headers: this.sdkKey ? { 'Authorization': `Bearer ${this.sdkKey}` } : {}
        });

        // Server sends "connected" before the full-state dump on every (re)connect.
        // Clear the Rust cache so stale / deleted flags are evicted first.
        this.sse.addEventListener('connected', (e) => {
            this.bridge.clearStore();
            this._flagCache.clear();
            // A successful connection resets the backoff schedule and ends any
            // active poll fallback — SSE deltas take over again.
            this._reconnectAttempts = 0;
            this._stopPollFallback();
            console.log('[Checkgate] Stream connected — cache cleared, rebuilding from server state.');

            // Capture the environment id so impressions route to the right endpoint.
            // Older servers send "true" (no id) — parsing simply yields no id.
            try {
                const payload = JSON.parse(e.data);
                this._envId = payload && payload.environment_id ? payload.environment_id : null;
            } catch {
                this._envId = null;
            }
            // Every (re)connect replays the full flag set as bootstrap "update"
            // events. Suppress change notifications until "ready" so listeners
            // only fire for genuine live deltas, not the initial/resync flood.
            this._bootstrapping = true;
            this._startImpressionTimer();
        });

        // Server sends "ready" once the full flag set has been replayed.
        this.sse.addEventListener('ready', () => {
            this._bootstrapping = false;
            this._hydrated = true;
            // Persist the freshly-bootstrapped full flag set for offline use.
            this._persistCache();
            if (!this._ready) {
                this._ready = true;
                if (this._resolveReady) {
                    this._resolveReady();
                    this._resolveReady = null;
                }
            }
        });

        this.sse.addEventListener('update', (e) => {
            try {
                const event = JSON.parse(e.data);

                if (event.type === 'UPSERT') {
                    // Pass the full flag object — the JSI layer JSON.stringifies it
                    // before crossing into Rust, preserving flag_type, default/disabled
                    // values, and per-rule variants for multi-variant flags.
                    this.bridge.upsertFlagV2(event.flag);
                    this._flagCache.set(event.flag.key, event.flag);
                    if (!this._bootstrapping) {
                        this._emitChange(event.flag.key);
                        this._persistCache();
                    }
                } else if (event.type === 'DELETE') {
                    this.bridge.deleteFlag(event.key);
                    this._flagCache.delete(event.key);
                    if (!this._bootstrapping) {
                        this._emitChange(event.key);
                        this._persistCache();
                    }
                }
            } catch (err) {
                console.error('[Checkgate] Failed to parse delta update:', err);
            }
        });

        this.sse.onerror = () => {
            // Take over reconnection so we can apply exponential backoff instead
            // of the EventSource's fixed retry cadence.
            this._scheduleReconnect();
        };
    }

    /**
     * Closes the current stream and schedules a reconnect using exponential
     * backoff with jitter, capped at maxReconnectDelayMs. No-op if the client
     * was closed by the caller or a reconnect is already pending.
     */
    _scheduleReconnect() {
        if (this._userClosed || this._reconnectTimer) return;
        if (this.sse) {
            this.sse.close();
            this.sse = null;
        }

        const base = this.reconnectDelayMs * Math.pow(2, this._reconnectAttempts);
        const capped = Math.min(this.maxReconnectDelayMs, base);
        const delay = capped + Math.random() * capped * 0.3; // up to 30% jitter
        this._reconnectAttempts++;

        // SSE is failing repeatedly (e.g. a proxy blocking long-lived connections) —
        // fall back to polling so flags still stay current. SSE retries continue in
        // the background; a successful reconnect stops the poll loop again.
        if (this._reconnectAttempts >= this.pollFallbackThreshold) {
            this._startPollFallback();
        }

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
        if (!this.bridge) return;
        try {
            const res = await fetch(`${this.serverUrl}/flags/snapshot`, {
                ...(controller ? { signal: controller.signal } : {}),
                headers: this.sdkKey ? { 'Authorization': `Bearer ${this.sdkKey}` } : {},
            });
            if (!res.ok) {
                console.warn(`[Checkgate] Poll fallback request failed: HTTP ${res.status}`);
                return;
            }
            const flags = await res.json();
            if (generation !== this._syncGeneration || this._userClosed) return;
            if (!Array.isArray(flags)) throw new Error('Snapshot was not an array of flags.');
            const envId = res.headers?.get('X-Checkgate-Environment-Id');
            if (envId) this._envId = envId;
            this._applySnapshot(flags);
            this._hydrated = true;
            this._startImpressionTimer();

            if (!this._ready) {
                this._ready = true;
                if (this._resolveReady) {
                    this._resolveReady();
                    this._resolveReady = null;
                }
            }
        } catch (err) {
            if (generation === this._syncGeneration && !this._userClosed) {
                console.warn('[Checkgate] Poll fallback request failed:', err && err.message);
            }
        }
    }

    /**
     * Applies a full flag snapshot, diffing against `_flagCache` so only flags
     * that actually changed (added, updated, or removed) trigger onChange and a
     * cache write. Returns true if anything changed.
     */
    _applySnapshot(flags) {
        const newKeys = new Set();
        let anyChange = false;

        for (const flag of flags) {
            newKeys.add(flag.key);
            const prev = this._flagCache.get(flag.key);
            if (prev && JSON.stringify(prev) === JSON.stringify(flag)) continue;

            // The JSI layer JSON.stringifies the object itself before crossing into
            // Rust, so pass the plain object here (unlike Node/Web which pass JSON).
            this.bridge.upsertFlagV2(flag);
            this._flagCache.set(flag.key, flag);
            anyChange = true;
            if (this._ready) this._emitChange(flag.key);
        }

        for (const key of Array.from(this._flagCache.keys())) {
            if (newKeys.has(key)) continue;
            this.bridge.deleteFlag(key);
            this._flagCache.delete(key);
            anyChange = true;
            if (this._ready) this._emitChange(key);
        }

        if (anyChange) this._persistCache();
        return anyChange;
    }

    /**
     * Evaluates a feature flag synchronously.
     * The call crosses JS → C++ JSI → Rust and returns in sub-microsecond time.
     *
     * @param {string} flagKey
     * @param {string} userKey        Stable user identifier used for rollout hashing.
     * @param {Object} [attributes]   Flat key→value map of user attributes for targeting rules.
     * @returns {boolean}
     */
    isEnabled(flagKey, userKey, attributes = {}) {
        if ((!this._ready && !this._hydrated) || !this.bridge) return false;
        const result = this.bridge.isEnabled(flagKey, userKey, attributes);
        this._recordImpression(flagKey, userKey, result ? 'true' : 'false', attributes);
        return result;
    }

    /**
     * Evaluates a multi-variant flag and returns the full result
     * `{ enabled, value }`, where `value` is the resolved variant
     * (boolean, string, number, or JSON object).
     *
     * @param {string} flagKey
     * @param {string} userKey
     * @param {Object} [attributes]
     * @returns {{ enabled: boolean, value: boolean|string|number|object|null } | null}
     *          `null` if the flag does not exist.
     */
    getVariant(flagKey, userKey, attributes = {}) {
        if ((!this._ready && !this._hydrated) || !this.bridge) return null;
        let variant;
        try {
            variant = JSON.parse(this.bridge.getVariant(flagKey, userKey, attributes));
        } catch (err) {
            console.error('[Checkgate] Failed to parse variant:', err);
            return null;
        }
        if (variant !== null) {
            this._recordImpression(
                flagKey, userKey, this._formatValue(variant.value), attributes);
        }
        return variant;
    }

    /**
     * Evaluates a multi-variant flag and returns just its resolved value.
     *
     * @param {string} flagKey
     * @param {string} userKey
     * @param {Object} [attributes]
     * @param {*} [defaultValue=null] - returned when the flag does not exist.
     * @returns {boolean|string|number|object|null}
     */
    getValue(flagKey, userKey, attributes = {}, defaultValue = null) {
        const variant = this.getVariant(flagKey, userKey, attributes);
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
            // Attributes are omitted unless explicitly opted in — the whole point
            // of local evaluation is that user attributes never leave the device.
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
        }).catch((err) => {
            console.warn('[Checkgate] Failed to report events:', err && err.message);
        });
    }

    /**
     * Tears down the SSE connection. Call in your cleanup effect.
     */
    disconnect() {
        // Stop managed reconnection before tearing down the stream.
        this._userClosed = true;
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
        if (this.sse) {
            this.sse.close();
            this.sse = null;
        }
        // Reset readiness so a later connect() re-establishes the stream.
        this._ready = false;
        this._bootstrapping = false;
        this._connectPromise = null;
        this._resolveReady = null;
        this._reconnectAttempts = 0;
    }
}
