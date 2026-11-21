'use strict';

// OpenFeature provider for Checkgate (server paradigm).
//
// Every evaluation is answered by the Checkgate client's local Rust engine
// (`getDetails`), the same code that answers `isEnabled`/`getVariant`: no
// network request per evaluation and no second copy of the targeting rules.
// The provider only translates — OpenFeature context in, OpenFeature
// resolution details out — and maps the client's connection state onto the
// OpenFeature provider lifecycle (READY / STALE / ERROR / FATAL).

const {
    OpenFeatureEventEmitter,
    ProviderEvents,
    ErrorCode,
    FlagNotFoundError,
    TypeMismatchError,
    TargetingKeyMissingError,
    ProviderNotReadyError,
    ProviderFatalError,
    GeneralError,
    StandardResolutionReasons,
} = require('@openfeature/server-sdk');

/** Checkgate reason → OpenFeature reason. */
const REASONS = {
    DISABLED: StandardResolutionReasons.DISABLED,
    // OpenFeature has no standard word for it; custom reasons are allowed.
    PREREQUISITE_FAILED: 'PREREQUISITE_FAILED',
    TARGETING_MATCH: StandardResolutionReasons.TARGETING_MATCH,
    ROLLOUT_EXCLUDED: StandardResolutionReasons.SPLIT,
    SPLIT: StandardResolutionReasons.SPLIT,
    DEFAULT: StandardResolutionReasons.DEFAULT,
    STATIC: StandardResolutionReasons.STATIC,
};

/** States in which the held flags may be going stale. */
const STALE_STATES = new Set(['connecting', 'reconnecting']);

/**
 * OpenFeature evaluation context → Checkgate attributes.
 *
 * Checkgate attributes are strings (numeric operators parse them), so
 * numbers and booleans are stringified, dates become ISO-8601, and values a
 * string cannot represent (objects, arrays, null) are left out — rather than
 * passed through to the engine, which rejects a non-string attribute a rule
 * reads. `targetingKey` is the user key, not an attribute.
 */
function toAttributes(context) {
    const attributes = {};
    for (const [name, value] of Object.entries(context || {})) {
        if (name === 'targetingKey' || value === undefined || value === null) continue;
        if (typeof value === 'string') attributes[name] = value;
        else if (typeof value === 'number') {
            if (Number.isFinite(value)) attributes[name] = String(value);
        } else if (typeof value === 'boolean') attributes[name] = String(value);
        else if (value instanceof Date) {
            if (!Number.isNaN(value.getTime())) attributes[name] = value.toISOString();
        }
    }
    return attributes;
}

function typeOf(value) {
    if (value === null) return 'null';
    if (Array.isArray(value)) return 'array';
    return typeof value;
}

/** Whether `value` is acceptable for a resolution of `expected` type. */
function matchesType(value, expected) {
    if (expected === 'object') return value !== null && typeof value === 'object';
    return typeof value === expected;
}

class CheckgateProvider {
    /**
     * @param {object} options
     *   Either `{ client }` — an existing `CheckgateClient` the application
     *   also uses directly (the provider never disconnects it) — or the
     *   `CheckgateClient` options (`serverUrl`, `sdkKey`, ...), in which case
     *   the provider creates, connects and on close disconnects its own client.
     * @param {number} [options.initTimeoutMs] - give up waiting for the first
     *   flag set after this long: `initialize()` rejects (provider ERROR)
     *   while the client keeps connecting in the background, and the provider
     *   turns READY when it succeeds. Default: wait as long as it takes
     *   (OpenFeature's `setProviderAndWait` then waits too).
     * @param {boolean} [options.requireTargetingKey=true] - resolve
     *   TARGETING_KEY_MISSING when the context has no `targetingKey`. With
     *   false, a missing key is evaluated as the empty string (fine for flags
     *   with no percentage rollout or weighted variants).
     */
    constructor(options = {}) {
        const { client, initTimeoutMs, requireTargetingKey = true, ...clientOptions } = options;
        this.metadata = { name: 'checkgate' };
        this.runsOn = 'server';
        this.hooks = [];
        this.events = new OpenFeatureEventEmitter();
        this._initTimeoutMs = initTimeoutMs;
        this._requireTargetingKey = requireTargetingKey;
        if (client) {
            this.client = client;
            this._ownsClient = false;
        } else {
            if (!clientOptions.serverUrl) {
                throw new Error('[Checkgate] CheckgateProvider needs `client` or `serverUrl`.');
            }
            // Required lazily so `{ client }` users need not resolve it twice.
            const { CheckgateClient } = require('@checkgate/node');
            this.client = new CheckgateClient(clientOptions);
            this._ownsClient = true;
        }
        this._initialized = false;
        this._closed = false;
        this._announced = null; // last lifecycle event emitted: 'ready' | 'stale' | 'error'
        this._staleSince = 0;
        this._pendingChanges = null;
        this._unsubscribe = [];
    }

    /** Called by OpenFeature once the provider is set; resolves → READY. */
    async initialize() {
        this._closed = false;
        this._watch();
        if (!this.client.isReady()) {
            try {
                await this._connect();
            } catch (err) {
                if (err && err.name === 'CheckgateAuthError') {
                    // A rejected key does not become valid by waiting: FATAL.
                    throw new ProviderFatalError(err.message);
                }
                throw new GeneralError(err && err.message ? err.message : String(err));
            }
        }
        this._initialized = true;
        this._announced = 'ready';
    }

    _connect() {
        const connecting = this.client.connect();
        if (typeof this._initTimeoutMs !== 'number') return connecting;
        let timer;
        const timeout = new Promise((_, reject) => {
            timer = setTimeout(() => reject(new Error(
                `[Checkgate] No flag set from the server within ${this._initTimeoutMs} ms; ` +
                'still connecting in the background.')), this._initTimeoutMs);
            if (timer.unref) timer.unref();
        });
        return Promise.race([connecting, timeout]).finally(() => clearTimeout(timer));
    }

    /** Subscribes to the client once: flag changes and connection state. */
    _watch() {
        if (this._unsubscribe.length > 0) return;
        this._unsubscribe.push(this.client.onChange((key) => this._queueChange(key)));
        this._unsubscribe.push(this.client.onStatusChange((status) => this._onStatus(status)));
    }

    /** Flag changes of one snapshot are reported as one event. */
    _queueChange(key) {
        if (this._closed) return;
        if (this._pendingChanges) {
            this._pendingChanges.add(key);
            return;
        }
        this._pendingChanges = new Set([key]);
        queueMicrotask(() => {
            const flagsChanged = Array.from(this._pendingChanges);
            this._pendingChanges = null;
            if (!this._closed) this.events.emit(ProviderEvents.ConfigurationChanged, { flagsChanged });
        });
    }

    _onStatus({ state, error, lastSyncAt }) {
        if (this._closed) return;
        // 'polling' is entered before any poll has succeeded; it shows
        // current flags only once a sync lands after the provider went stale.
        const fresh = state === 'streaming' ||
            (state === 'polling' && lastSyncAt !== null && lastSyncAt >= this._staleSince);
        if (fresh && this.client.isReady()) {
            // Recovered (or became ready after a timed-out initialize).
            if (this._announced !== 'ready') {
                this._initialized = true;
                this._announce('ready', ProviderEvents.Ready, {});
            }
        } else if ((STALE_STATES.has(state) || state === 'polling') && this._initialized) {
            if (this._announced !== 'stale') this._staleSince = Date.now();
            this._announce('stale', ProviderEvents.Stale, {
                message: `Checkgate connection ${state}; serving the last known flags.`,
            });
        } else if (state === 'auth_failed' && this._initialized) {
            // Flags already held keep being served, so not FATAL: evaluations
            // continue while the key is fixed and connect() retried.
            this._announce('error', ProviderEvents.Error, {
                message: error && error.message ? error.message : 'SDK key rejected',
                errorCode: ErrorCode.GENERAL,
            });
        }
    }

    _announce(kind, event, details) {
        if (this._announced === kind) return;
        this._announced = kind;
        this.events.emit(event, details);
    }

    async onClose() {
        this._closed = true;
        for (const off of this._unsubscribe.splice(0)) off();
        this._pendingChanges = null;
        if (this._ownsClient) this.client.disconnect();
        this._initialized = false;
        this._announced = null;
    }

    resolveBooleanEvaluation(flagKey, defaultValue, context, logger) {
        return this._resolve(flagKey, defaultValue, context, 'boolean', logger);
    }

    resolveStringEvaluation(flagKey, defaultValue, context, logger) {
        return this._resolve(flagKey, defaultValue, context, 'string', logger);
    }

    resolveNumberEvaluation(flagKey, defaultValue, context, logger) {
        return this._resolve(flagKey, defaultValue, context, 'number', logger);
    }

    resolveObjectEvaluation(flagKey, defaultValue, context, logger) {
        return this._resolve(flagKey, defaultValue, context, 'object', logger);
    }

    /**
     * OpenFeature tracking → a Checkgate goal event (A/B conversions).
     * `trackingEventDetails.value` becomes the event value; the remaining
     * details are its context.
     */
    track(trackingEventName, context, trackingEventDetails = {}) {
        const key = context && context.targetingKey;
        if (typeof key !== 'string' || key === '') return;
        const { value, ...rest } = trackingEventDetails || {};
        this.client.track(trackingEventName, key, {
            ...(typeof value === 'number' ? { value } : {}),
            ...(Object.keys(rest).length > 0 ? { context: rest } : {}),
        });
    }

    // Async to satisfy the interface; resolves on the same tick (no I/O).
    async _resolve(flagKey, defaultValue, context, expected) {
        if (!this._initialized) throw new ProviderNotReadyError('Checkgate provider is not initialized.');
        const targetingKey = context && context.targetingKey;
        if (typeof targetingKey !== 'string' || targetingKey === '') {
            if (this._requireTargetingKey) {
                throw new TargetingKeyMissingError('Checkgate needs `targetingKey` in the evaluation context.');
            }
        }
        const details = this.client.getDetails(
            flagKey, typeof targetingKey === 'string' ? targetingKey : '', toAttributes(context));
        if (details === null) throw new FlagNotFoundError(`Flag '${flagKey}' not found.`);

        const reason = REASONS[details.reason] || details.reason;
        const flagMetadata = { checkgateReason: details.reason };
        if (details.ruleIndex !== undefined) flagMetadata.ruleIndex = details.ruleIndex;
        if (details.variantIndex !== undefined) flagMetadata.variantIndex = details.variantIndex;
        if (details.rolloutBucket !== undefined) flagMetadata.rolloutBucket = details.rolloutBucket;

        const value = details.value;
        if (!matchesType(value, expected)) {
            // A flag that is off (or on with no value configured) and has no
            // value of the requested type means "use the caller's default" —
            // not a type error.
            if (!details.enabled || value === null) {
                return { value: defaultValue, reason, flagMetadata };
            }
            throw new TypeMismatchError(
                `Flag '${flagKey}' resolved to a ${typeOf(value)}, not a ${expected}.`);
        }
        const resolution = { value, reason, flagMetadata };
        if (typeof value !== 'object') resolution.variant = String(value);
        return resolution;
    }
}

module.exports = { CheckgateProvider, toAttributes, REASONS };
