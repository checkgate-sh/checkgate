// Express adapter against the real @checkgate/node client and the fake
// Checkgate server (see helpers.mjs).

import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { checkResponse, contextFromHeaders, frameworkSuite, get, makeClient, quietLogger, startFake, userFor } from './helpers.mjs'

const require = createRequire(import.meta.url)
const express = require('express')
const { checkgateMiddleware, listenWithCheckgate } = require('@checkgate/node-integrations/express')

function buildApp(client, context = (req) => contextFromHeaders(req.headers)) {
    const app = express()
    app.use(checkgateMiddleware({ client, context }))
    app.get('/check', async (req, res) => {
        res.json(await checkResponse(req.checkgate))
    })
    return app
}

frameworkSuite('express', {
    async start({ client, readyTimeoutMs, onTimeout }) {
        const { server, close } = await listenWithCheckgate(buildApp(client), {
            client, host: '127.0.0.1', readyTimeoutMs, onTimeout, logger: quietLogger,
        })
        return { url: `http://127.0.0.1:${server.address().port}`, close }
    },
})

test('[express] context resolves lazily: auth middleware mounted after Checkgate still counts', async () => {
    const fake = await startFake()
    const client = makeClient(fake.url)
    const app = express()
    // Default extractor: req.user.id.
    app.use(checkgateMiddleware({ client }))
    app.use((req, _res, next) => {
        if (req.headers['x-user']) req.user = { id: req.headers['x-user'] }
        next()
    })
    app.get('/whoami', (req, res) => res.json({ key: req.checkgate.context.key }))
    const { server, close } = await listenWithCheckgate(app, { client, host: '127.0.0.1', logger: quietLogger })
    try {
        const url = `http://127.0.0.1:${server.address().port}`
        assert.equal((await get(url, '/whoami', { 'x-user': 'u-42' })).body.key, 'u-42')
        assert.equal((await get(url, '/whoami')).body.key, 'anonymous')
    } finally {
        await close()
        await fake.stop()
    }
})

test('[express] a throwing context extractor serves the anonymous context and reports the error', async () => {
    const fake = await startFake()
    const client = makeClient(fake.url)
    const errors = []
    const app = express()
    app.use(checkgateMiddleware({
        client,
        context: () => { throw new Error('extractor bug') },
        onError: (err, info) => errors.push({ message: err.message, info }),
    }))
    app.get('/check', async (req, res) => res.json(await checkResponse(req.checkgate)))
    const { server, close } = await listenWithCheckgate(app, { client, host: '127.0.0.1', logger: quietLogger })
    try {
        const res = await get(`http://127.0.0.1:${server.address().port}`, '/check', userFor(0).headers)
        assert.equal(res.status, 200)
        assert.equal(res.body.key, 'anonymous')
        assert.equal(res.body.pro, false)
        // Resolved once per request, so reported once.
        assert.deepEqual(errors, [{ message: 'extractor bug', info: { phase: 'context' } }])
    } finally {
        await close()
        await fake.stop()
    }
})

test('[express] checkgateMiddleware() rejects a missing client at setup', () => {
    assert.throws(() => checkgateMiddleware({}), /needs \{ client \}/)
    assert.throws(() => checkgateMiddleware({ client: makeClient('http://127.0.0.1:9'), context: 'nope' }), /context must be a function/)
})
