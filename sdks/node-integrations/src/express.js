'use strict'

// Express adapter: `req.checkgate` per request, plus a startup/shutdown helper
// for the HTTP server. Uses nothing from Express itself, so the middleware
// also fits any (req, res, next) stack.

const http = require('node:http')
const { createRequestFlags, defaultContext, startCheckgate, stopCheckgate } = require('./core.js')

function requireClient(client, name) {
    if (!client || typeof client.isEnabled !== 'function') {
        throw new TypeError(`[Checkgate] ${name} needs { client }: one CheckgateClient shared by the whole app.`)
    }
}

/**
 * Middleware that sets `req.checkgate`, the flags for this request's user.
 * `context(req)` is called once, on the first evaluation, so it can read
 * `req.user` set by auth middleware mounted after this one.
 *
 * @param {object} options
 * @param {object} options.client - the one shared CheckgateClient
 * @param {(req: object) => unknown} [options.context] - returns `{ key, attributes }`;
 *        default: `{ key: req.user.id }`, else anonymous
 * @param {string} [options.defaultKey='anonymous'] - user key when the context has none
 * @param {(err: unknown, info: object) => void} [options.onError] - evaluation errors
 *        (default: logged to console.error); the evaluation returns its default
 */
function checkgateMiddleware({ client, context = defaultContext, defaultKey, onError } = {}) {
    requireClient(client, 'checkgateMiddleware()')
    if (typeof context !== 'function') throw new TypeError('[Checkgate] context must be a function (req) => { key, attributes }.')
    const options = { defaultKey, onError }
    return function checkgate(req, _res, next) {
        req.checkgate = createRequestFlags(client, () => context(req), options)
        next()
    }
}

/**
 * Starts Checkgate, then the HTTP server, and returns a `close()` that stops
 * both in order: the server stops accepting connections and finishes the
 * requests in flight (with flags still served), then the client disconnects
 * exactly once. `close()` is idempotent.
 *
 * The helper does not install signal handlers; wire them yourself:
 *
 *     const { close } = await listenWithCheckgate(app, { client, port: 3000 })
 *     process.once('SIGTERM', () => close().then(() => process.exit(0)))
 *
 * @param {(req: object, res: object) => void} app - an Express app (any request listener)
 * @param {object} options
 * @param {object} options.client
 * @param {number} [options.port=0]
 * @param {string} [options.host]
 * @param {number} [options.readyTimeoutMs=5000]
 * @param {'continue'|'fail'} [options.onTimeout='continue']
 * @param {{ warn(...args: unknown[]): void }} [options.logger=console]
 */
async function listenWithCheckgate(app, { client, port = 0, host, readyTimeoutMs, onTimeout, logger } = {}) {
    requireClient(client, 'listenWithCheckgate()')
    const startup = await startCheckgate(client, { readyTimeoutMs, onTimeout, logger })
    const server = http.createServer(app)
    try {
        await new Promise((resolve, reject) => {
            server.once('error', reject)
            const done = () => {
                server.off('error', reject)
                resolve()
            }
            if (host === undefined) server.listen(port, done)
            else server.listen(port, host, done)
        })
    } catch (err) {
        stopCheckgate(client)
        throw err
    }

    let closing = null
    const close = () => {
        if (!closing) {
            closing = new Promise((resolve) => {
                // Resolves once every connection has ended; an error only
                // means the server was not listening any more.
                server.close(() => resolve())
                // Keep-alive sockets with no request in flight would hold
                // close() open until they time out.
                if (typeof server.closeIdleConnections === 'function') server.closeIdleConnections()
            }).finally(() => {
                stopCheckgate(client)
            })
        }
        return closing
    }
    return { server, startup, close }
}

module.exports = { checkgateMiddleware, listenWithCheckgate, startCheckgate, stopCheckgate }
