'use strict'

/**
 * An in-memory stand-in for CheckgateClient, for application tests: no
 * server, no native addon, no network. Pass it as `client` to any adapter
 * (or use `CheckgateModule.forTesting()` in NestJS).
 *
 * `flags` maps a flag key to its value for every user, or to a function
 * `({ key, attributes }) => value` for per-user answers:
 *
 * - `true` / `false`: a boolean flag that is on / off;
 * - any other value (string, number, object): the flag is on and serves it;
 * - `null`: the flag is off.
 *
 * A key that is not in `flags` behaves like a missing flag: `isEnabled` is
 * `false`, `getVariant` is `null`, `getValue` returns the default.
 *
 * @param {Record<string, unknown>} [flags]
 */
function createTestClient(flags = {}) {
    const values = new Map(Object.entries(flags))
    let connected = false
    const resolve = (flag, key, attributes) => {
        const value = values.get(flag)
        return typeof value === 'function' ? value({ key, attributes }) : value
    }
    const client = {
        /** Calls to disconnect(), for assertions. */
        disconnectCalls: 0,
        /** Goal events passed to track(), for assertions. */
        tracked: [],
        connect() {
            connected = true
            return Promise.resolve()
        },
        disconnect() {
            connected = false
            client.disconnectCalls++
        },
        isReady: () => connected,
        isPolling: () => false,
        getStatus: () => ({ state: connected ? 'streaming' : 'idle', lastSyncAt: connected ? Date.now() : null, error: null }),
        onChange: () => () => {},
        isEnabled(flag, key, attributes = {}) {
            if (!values.has(flag)) return false
            const value = resolve(flag, key, attributes)
            return value !== false && value !== null && value !== undefined
        },
        getVariant(flag, key, attributes = {}) {
            if (!values.has(flag)) return null
            const value = resolve(flag, key, attributes)
            return { enabled: value !== false && value !== null && value !== undefined, value: value === undefined ? null : value }
        },
        getValue(flag, key, attributes = {}, defaultValue = null) {
            const variant = client.getVariant(flag, key, attributes)
            return variant === null ? defaultValue : variant.value
        },
        track(eventKey, key, opts) {
            client.tracked.push({ eventKey, key, opts })
        },
        /** Changes a flag (or removes it with `undefined`) for the rest of the test. */
        setFlag(flag, value) {
            if (value === undefined) values.delete(flag)
            else values.set(flag, value)
        },
    }
    return client
}

module.exports = { createTestClient }
