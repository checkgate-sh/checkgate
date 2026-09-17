const { cacheNamespace } = require('./cache-key.js');
// `native-binding.js` is the napi-rs-generated multi-platform dispatch loader
// (built via `--js native-binding.js`, kept out of `index.js` so it doesn't
// collide with this hand-written wrapper) — it picks the correct
// `checkgate.<platform>.node` for the current OS/arch/libc at require time.
const { CheckgateCore } = require('./native-binding.js');
// eventsource v4 exports EventSource as a named export, not a default export.
const { EventSource } = require('eventsource');

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
        this._bootstrapping = false;
        this._connectPromise = null;
        this._resolveReady = null;
        this._changeListeners = [];
        this.sse = null;

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
     * @returns {Promise<void>}
     */
    connect() {
        if (this._ready) return Promise.resolve();
        if (this._connectPromise) return this._connectPromise;

        this._userClosed = false;
        this._connectPromise = new Promise((resolve) => {
            this._resolveReady = resolve;
        });
        // Hydrate from the persisted snapshot first so evaluations work offline
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

    _connectDeltas() {
        if (this.sse) {
            this.sse.close();
        }

        // eventsource v4 dropped the plain `headers` option in favor of a custom
        // `fetch` hook — passing `headers` directly (as older versions accepted)
        // is silently ignored, so the Bearer token would never actually be sent.
        this.sse = new EventSource(`${this.serverUrl}/stream`, {
            fetch: (url, init) => fetch(url, {
                ...init,
                headers: {
                    ...init.headers,
                    ...(this.sdkKey ? { 'Authorization': `Bearer ${this.sdkKey}` } : {}),
                },
            }),
        });

        // Server sends "connected" before the full-state dump on every (re)connect.
        // Clear the cache so stale/deleted flags from the previous session are evicted.
        this.sse.addEventListener('connected', (e) => {
            this.core.clearStore();
            this._flagCache.clear();
            // A successful connection resets the backoff schedule and ends any
            // active poll fallback — SSE deltas take over again.
            this._reconnectAttempts = 0;
            this._stopPollFallback();
            console.log('[Checkgate] Stream connected — cache cleared, rebuilding from server state.');

            // Capture the environment id so impressions can be routed to the
            // correct ingest endpoint. Older servers send "true" (no id).
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
                    // Pass the full flag (flag_type, default/disabled values, and
                    // per-rule variants) so getValue()/getVariant() resolve
                    // non-boolean flags correctly.
                    this.core.upsertFlagV2(JSON.stringify(event.flag));
                    this._flagCache.set(event.flag.key, event.flag);
                    if (!this._bootstrapping) {
                        this._emitChange(event.flag.key);
                        this._persistCache(); // keep the offline snapshot fresh
                    }
                } else if (event.type === 'DELETE') {
                    this.core.deleteFlag(event.key);
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
            // of the library's fixed retry cadence.
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

            this.core.upsertFlagV2(JSON.stringify(flag));
            this._flagCache.set(flag.key, flag);
            anyChange = true;
            if (this._ready) this._emitChange(flag.key);
        }

        for (const key of Array.from(this._flagCache.keys())) {
            if (newKeys.has(key)) continue;
            this.core.deleteFlag(key);
            this._flagCache.delete(key);
            anyChange = true;
            if (this._ready) this._emitChange(key);
        }

        if (anyChange) this._persistCache();
        return anyChange;
    }

    /**
     * Evaluates a feature flag synchronously in <1 microsecond with 0 network IO.
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

    /**
     * Closes the SSE connection and cleans up resources.
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

module.exports = { CheckgateClient };
