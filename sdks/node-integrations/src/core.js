'use strict'

// Framework-independent pieces shared by the Express, Fastify and NestJS
// adapters: the request-bound evaluator, attribute normalization, and the
// startup/shutdown lifecycle around one shared CheckgateClient.
//
// Nothing here stores per-request data on the client, and nothing here does
// network I/O: an evaluation is a synchronous call into the client's in-memory
// flag store.

const DEFAULT_KEY = 'anonymous'

/**
 * Converts one attribute value to the string the evaluation core expects, or
 * `undefined` to drop it. The native core throws when an attribute a flag rule
 * references is not a string; coercing here keeps that error out of requests.
 *
 * - strings are kept as they are;
 * - finite numbers and bigints become their decimal string (`30` -> `"30"`), so
 *   numeric rule operators (`greater_than`, ...) work on them;
 * - booleans become `"true"` / `"false"`;
 * - everything else is dropped: `null`, `undefined`, `NaN`, `Infinity`,
 *   objects, arrays, dates, symbols, functions. A rule on a dropped attribute
 *   evaluates as if the attribute were absent.
 */
function toAttributeValue(value) {
    switch (typeof value) {
        case 'string':
            return value
        case 'number':
            return Number.isFinite(value) ? String(value) : undefined
        case 'bigint':
            return value.toString()
        case 'boolean':
            return value ? 'true' : 'false'
        default:
            return undefined
    }
}

/**
 * A fresh `Record<string, string>` holding the coercible attributes of
 * `input` (own enumerable string keys only). Never throws; anything that is
 * not an object yields `{}`.
 */
function normalizeAttributes(input) {
    const out = {}
    if (input === null || typeof input !== 'object') return out
    let keys
    try {
        keys = Object.keys(input)
    } catch {
        return out
    }
    for (const key of keys) {
        // Would set the prototype of `out` instead of a property.
        if (key === '__proto__') continue
        let raw
        try {
            raw = input[key]
        } catch {
            continue
        }
        const value = toAttributeValue(raw)
        if (value !== undefined) out[key] = value
    }
    return out
}

/** The user key as a non-empty string, or `fallback`. */
function normalizeKey(key, fallback) {
    if (typeof key === 'string') return key.length > 0 ? key : fallback
    if (typeof key === 'number') return Number.isFinite(key) ? String(key) : fallback
    if (typeof key === 'bigint') return key.toString()
    return fallback
}

/**
 * Normalizes what a `context(req)` extractor returned: `{ key, attributes }`,
 * a bare key string, or nothing (anonymous). Returns a frozen
 * `{ key: string, attributes: Record<string, string> }`.
 */
function normalizeContext(raw, defaultKey = DEFAULT_KEY) {
    if (raw === null || raw === undefined) {
        return Object.freeze({ key: defaultKey, attributes: Object.freeze({}) })
    }
    if (typeof raw !== 'object') {
        return Object.freeze({ key: normalizeKey(raw, defaultKey), attributes: Object.freeze({}) })
    }
    return Object.freeze({
        key: normalizeKey(raw.key, defaultKey),
        attributes: Object.freeze(normalizeAttributes(raw.attributes)),
    })
}

/**
 * The extractor used when none is given: the request's authenticated user id
 * (`req.user.id`, as Passport and most auth middleware set it), else anonymous.
 */
function defaultContext(req) {
    const user = req && req.user
    return user && typeof user === 'object' ? { key: user.id } : null
}

function defaultOnError(err, info) {
    const where = info && info.flag !== undefined ? ` evaluating "${String(info.flag)}"` : ` (${info && info.phase})`
    console.error(`[Checkgate] Integration error${where}; serving the default.`, err)
}

/**
 * Flags evaluated for one request. Created per request from the one shared
 * client; holds only that request's context, which is resolved from
 * `resolveContext()` on first use (so auth middleware that runs after the
 * Checkgate middleware has set `req.user` by then) and then reused for every
 * evaluation of the request.
 *
 * No method throws: a failure (an extractor that throws, a malformed flag key)
 * is reported to `onError` and the evaluation returns its default.
 */
class RequestFlags {
    #client
    #resolveContext
    #defaultKey
    #onError
    #context = null

    /**
     * @param {object} client - a CheckgateClient (or anything with its evaluation methods)
     * @param {() => unknown} resolveContext - returns `{ key, attributes }`, a key, or nothing
     * @param {{ defaultKey?: string, onError?: (err: unknown, info: object) => void }} [options]
     */
    constructor(client, resolveContext, { defaultKey = DEFAULT_KEY, onError = defaultOnError } = {}) {
        this.#client = client
        this.#resolveContext = resolveContext
        this.#defaultKey = defaultKey
        this.#onError = onError
    }

    /** The normalized `{ key, attributes }` this request evaluates with. */
    get context() {
        if (this.#context === null) {
            let raw = null
            try {
                raw = typeof this.#resolveContext === 'function' ? this.#resolveContext() : this.#resolveContext
            } catch (err) {
                this.#report(err, { phase: 'context' })
                raw = null
            }
            this.#context = normalizeContext(raw, this.#defaultKey)
        }
        return this.#context
    }

    /**
     * Whether the client can answer from flags yet. Before the first
     * bootstrap and without an offline cache, the SDK would return the same
     * defaults but log a warning per call; skipping the call keeps a server
     * that started without Checkgate (`onTimeout: 'continue'`) from logging
     * one line per request. `_hydrated` is the SDK's own flag for "flags are
     * loaded from the offline cache"; a client without it is always asked.
     */
    #servable() {
        const client = this.#client
        if (typeof client.isReady !== 'function' || client.isReady()) return true
        return client._hydrated !== false
    }

    #report(err, info) {
        try {
            this.#onError(err, info)
        } catch {
            // A throwing error handler must not take the request down either.
        }
    }

    /** `true` if `flag` is on for this request's user; `false` if it is off, missing, or on error. */
    isEnabled(flag) {
        try {
            if (!this.#servable()) return false
            const { key, attributes } = this.context
            return this.#client.isEnabled(flag, key, attributes) === true
        } catch (err) {
            this.#report(err, { flag })
            return false
        }
    }

    /** `{ enabled, value }` for this request's user, or `null` if the flag is missing (or on error). */
    getVariant(flag) {
        try {
            if (!this.#servable()) return null
            const { key, attributes } = this.context
            const variant = this.#client.getVariant(flag, key, attributes)
            return variant === undefined ? null : variant
        } catch (err) {
            this.#report(err, { flag })
            return null
        }
    }

    /** The resolved value for this request's user, or `defaultValue` if the flag is missing (or on error). */
    getValue(flag, defaultValue = null) {
        const variant = this.getVariant(flag)
        return variant === null ? defaultValue : variant.value
    }

    /** Records a goal event for this request's user (see `CheckgateClient.track`). */
    track(eventKey, opts) {
        try {
            if (typeof this.#client.track === 'function') this.#client.track(eventKey, this.context.key, opts)
        } catch (err) {
            this.#report(err, { phase: 'track' })
        }
    }
}

/**
 * The evaluator for one request.
 *
 * @param {object} client - the one shared CheckgateClient
 * @param {() => unknown} resolveContext - called at most once, on first use
 * @param {object} [options] - `{ defaultKey, onError }`
 */
function createRequestFlags(client, resolveContext, options) {
    return new RequestFlags(client, resolveContext, options)
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/** Raised by startCheckgate() with `onTimeout: 'fail'` when the client was not ready in time. */
class CheckgateStartupTimeoutError extends Error {
    constructor(timeoutMs, state) {
        super(`[Checkgate] Flags were not ready within ${timeoutMs} ms (connection state: ${state}). ` +
            "Failing startup because onTimeout is 'fail'.")
        this.name = 'CheckgateStartupTimeoutError'
        this.timeoutMs = timeoutMs
        this.state = state
    }
}

// Clients stopCheckgate() has disconnected and startCheckgate() has not
// started again since. Keyed weakly so a dropped client is not kept alive.
const stopped = new WeakSet()

function stateOf(client) {
    try {
        return typeof client.getStatus === 'function' ? client.getStatus().state : 'unknown'
    } catch {
        return 'unknown'
    }
}

/**
 * Connects `client` for an application that is starting.
 *
 * `client.connect()` keeps retrying while the server is unreachable, so this
 * waits at most `readyTimeoutMs`. Then, with `onTimeout: 'continue'`
 * (default), it resolves `{ ready: false, timedOut: true }` and the client
 * keeps syncing in the background, serving its offline cache (if a `storage`
 * adapter has one) or defaults until then; with `'fail'` it disconnects the
 * client and rejects with a CheckgateStartupTimeoutError.
 *
 * A rejected SDK key (CheckgateAuthError) always rejects, whatever
 * `onTimeout` says: it will not fix itself.
 *
 * @param {object} client
 * @param {{ readyTimeoutMs?: number, onTimeout?: 'continue'|'fail', logger?: { warn(...args: unknown[]): void } }} [options]
 * @returns {Promise<{ ready: boolean, timedOut: boolean }>}
 */
async function startCheckgate(client, { readyTimeoutMs = 5000, onTimeout = 'continue', logger = console } = {}) {
    if (!client || typeof client.connect !== 'function') {
        throw new TypeError('[Checkgate] startCheckgate() needs a CheckgateClient.')
    }
    if (onTimeout !== 'continue' && onTimeout !== 'fail') {
        throw new TypeError(`[Checkgate] onTimeout must be 'continue' or 'fail', got ${String(onTimeout)}.`)
    }
    if (typeof readyTimeoutMs !== 'number' || !(readyTimeoutMs >= 0)) {
        throw new TypeError('[Checkgate] readyTimeoutMs must be a number >= 0.')
    }
    stopped.delete(client)

    const READY = Symbol('ready')
    const TIMEOUT = Symbol('timeout')
    let timer = null
    const timeout = new Promise((resolve) => {
        timer = setTimeout(resolve, readyTimeoutMs, TIMEOUT)
    })
    let outcome
    try {
        // Rejects on a rejected key, or when the client is disconnected first.
        const connected = Promise.resolve().then(() => client.connect()).then(() => READY)
        outcome = await Promise.race([connected, timeout])
    } finally {
        clearTimeout(timer)
    }
    if (outcome === READY) return { ready: true, timedOut: false }

    const state = stateOf(client)
    if (onTimeout === 'fail') {
        stopCheckgate(client)
        throw new CheckgateStartupTimeoutError(readyTimeoutMs, state)
    }
    if (logger && typeof logger.warn === 'function') {
        logger.warn(`[Checkgate] Flags not ready within ${readyTimeoutMs} ms (state: ${state}); ` +
            'starting anyway with cached or default values, still syncing in the background.')
    }
    return { ready: false, timedOut: true }
}

/**
 * Disconnects `client` once: later calls (a second close, a shutdown hook
 * after a failed start) do nothing until startCheckgate() starts it again.
 * Returns whether this call disconnected it.
 */
function stopCheckgate(client) {
    if (!client || stopped.has(client)) return false
    stopped.add(client)
    if (typeof client.disconnect === 'function') client.disconnect()
    return true
}

module.exports = {
    DEFAULT_KEY,
    RequestFlags,
    CheckgateStartupTimeoutError,
    createRequestFlags,
    defaultContext,
    normalizeAttributes,
    normalizeContext,
    startCheckgate,
    stopCheckgate,
    toAttributeValue,
}
