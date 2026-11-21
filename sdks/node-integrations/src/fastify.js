'use strict'

// Fastify plugin: `fastify.checkgate` (the shared client) and
// `request.checkgate` (the flags for that request's user). The client connects
// while the plugin loads, so `fastify.ready()` / `listen()` wait for it (up to
// `readyTimeoutMs`), and disconnects in `onClose`.

const fp = require('fastify-plugin')
const { createRequestFlags, defaultContext, startCheckgate, stopCheckgate } = require('./core.js')

/**
 * @param {import('fastify').FastifyInstance} fastify
 * @param {object} opts
 * @param {object} [opts.client] - a CheckgateClient; or
 * @param {object} [opts.clientOptions] - options to construct one (the plugin then owns it)
 * @param {(request: object) => unknown} [opts.context] - returns `{ key, attributes }`;
 *        default: `{ key: request.user.id }`, else anonymous. Called once per request, on first use.
 * @param {string} [opts.defaultKey='anonymous']
 * @param {(err: unknown, info: object) => void} [opts.onError]
 * @param {number} [opts.readyTimeoutMs=5000]
 * @param {'continue'|'fail'} [opts.onTimeout='continue']
 */
async function checkgatePlugin(fastify, opts) {
    const { client: given, clientOptions, context = defaultContext, defaultKey, onError, readyTimeoutMs, onTimeout } = opts || {}
    if (given && clientOptions) throw new TypeError('[Checkgate] Pass either client or clientOptions, not both.')
    if (!given && !clientOptions) throw new TypeError('[Checkgate] The Fastify plugin needs client or clientOptions.')
    if (typeof context !== 'function') throw new TypeError('[Checkgate] context must be a function (request) => { key, attributes }.')
    // Fastify aborts a plugin that takes longer than `pluginTimeout` (10 s by
    // default) with a timeout error of its own, which would hide the
    // onTimeout policy.
    const pluginTimeout = fastify.initialConfig && fastify.initialConfig.pluginTimeout
    const waitMs = readyTimeoutMs === undefined ? 5000 : readyTimeoutMs
    if (typeof pluginTimeout === 'number' && pluginTimeout > 0 && waitMs >= pluginTimeout) {
        throw new TypeError(`[Checkgate] readyTimeoutMs (${waitMs}) must be below Fastify's pluginTimeout (${pluginTimeout}).`)
    }
    // Loaded here, not at the top: an app that passes its own client (or a
    // test client) does not need the native addon to load this module.
    const client = given || new (require('@checkgate/node').CheckgateClient)(clientOptions)

    const options = { defaultKey, onError }
    fastify.decorate('checkgate', client)
    // Fastify 5: reference-type request decorators must start as null and be
    // assigned per request, or every request would share one object.
    fastify.decorateRequest('checkgate', null)
    fastify.addHook('onRequest', function checkgateRequest(request, _reply, done) {
        request.checkgate = createRequestFlags(client, () => context(request), options)
        done()
    })
    // Registered before connecting, so a close after a failed start still
    // disconnects (once).
    fastify.addHook('onClose', async function checkgateClose() {
        stopCheckgate(client)
    })

    await startCheckgate(client, { readyTimeoutMs, onTimeout, logger: fastify.log })
}

const plugin = fp(checkgatePlugin, { fastify: '5.x', name: 'checkgate' })

module.exports = plugin
module.exports.default = plugin
module.exports.checkgatePlugin = plugin
module.exports.startCheckgate = startCheckgate
module.exports.stopCheckgate = stopCheckgate
