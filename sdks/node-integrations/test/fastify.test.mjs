// Fastify plugin against the real @checkgate/node client and the fake
// Checkgate server (see helpers.mjs).

import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { SDK_KEY, checkResponse, contextFromHeaders, frameworkSuite, get, makeClient, startFake, waitFor } from './helpers.mjs'

const require = createRequire(import.meta.url)
const Fastify = require('fastify')
const checkgatePlugin = require('@checkgate/node-integrations/fastify')

async function buildApp(pluginOptions) {
    const app = Fastify({ logger: false })
    await app.register(checkgatePlugin, { context: (request) => contextFromHeaders(request.headers), ...pluginOptions })
    app.get('/check', async (request) => checkResponse(request.checkgate))
    return app
}

frameworkSuite('fastify', {
    async start({ client, readyTimeoutMs, onTimeout }) {
        const app = Fastify({ logger: false })
        app.register(checkgatePlugin, {
            client, readyTimeoutMs, onTimeout, context: (request) => contextFromHeaders(request.headers),
        })
        app.get('/check', async (request) => checkResponse(request.checkgate))
        try {
            await app.listen({ port: 0, host: '127.0.0.1' })
        } catch (err) {
            await app.close().catch(() => {})
            throw err
        }
        return { url: `http://127.0.0.1:${app.server.address().port}`, close: () => app.close() }
    },
})

test('[fastify] decorates the instance with the shared client and each request with its own flags', async () => {
    const fake = await startFake()
    const client = makeClient(fake.url)
    const app = Fastify({ logger: false })
    await app.register(checkgatePlugin, { client, context: (request) => contextFromHeaders(request.headers) })
    const seen = []
    app.get('/same', async (request) => {
        seen.push(request.checkgate)
        return { shared: app.checkgate === client }
    })
    await app.listen({ port: 0, host: '127.0.0.1' })
    try {
        const url = `http://127.0.0.1:${app.server.address().port}`
        assert.equal((await get(url, '/same')).body.shared, true)
        await get(url, '/same')
        assert.equal(seen.length, 2)
        assert.notEqual(seen[0], seen[1], 'a fresh evaluator per request')
    } finally {
        await app.close()
        await fake.stop()
    }
})

test('[fastify] the plugin is not encapsulated: routes in child plugins see request.checkgate', async () => {
    const fake = await startFake()
    const app = await buildApp({ client: makeClient(fake.url) })
    app.register(async (child) => {
        child.get('/child', async (request) => ({ key: request.checkgate.context.key }))
    }, { prefix: '/nested' })
    await app.listen({ port: 0, host: '127.0.0.1' })
    try {
        const res = await get(`http://127.0.0.1:${app.server.address().port}`, '/nested/child', { 'x-user': 'kid' })
        assert.equal(res.body.key, 'kid')
    } finally {
        await app.close()
        await fake.stop()
    }
})

test('[fastify] clientOptions: the plugin builds, connects and closes its own client', async () => {
    const fake = await startFake()
    const app = await buildApp({
        clientOptions: { serverUrl: fake.url, sdkKey: SDK_KEY, reportImpressions: false },
        readyTimeoutMs: 3000,
        onTimeout: 'fail',
    })
    await app.listen({ port: 0, host: '127.0.0.1' })
    assert.equal(app.checkgate.isReady(), true)
    const res = await get(`http://127.0.0.1:${app.server.address().port}`, '/check', { 'x-user': 'p', 'x-plan': 'pro' })
    assert.equal(res.body.pro, true)
    await app.close()
    assert.equal(app.checkgate.getStatus().state, 'closed')
    await waitFor(() => fake.streams.size === 0, { label: 'the stream to close' })
    await fake.stop()
})

test('[fastify] option errors surface from register()', async () => {
    await assert.rejects(buildApp({}), /needs client or clientOptions/)
    await assert.rejects(buildApp({ client: makeClient('http://127.0.0.1:9'), clientOptions: {} }), /either client or clientOptions/)
    // A wait longer than Fastify's own plugin timeout would end in Fastify's error, not ours.
    const app = Fastify({ logger: false, pluginTimeout: 1000 })
    app.register(checkgatePlugin, { client: makeClient('http://127.0.0.1:9'), readyTimeoutMs: 1000 })
    await assert.rejects(app.ready(), /must be below Fastify's pluginTimeout/)
})
