// The framework-independent core: attribute coercion against the real native
// core, the request evaluator's no-throw contract, the startup/shutdown
// helpers, the test client, and the shared evaluation vectors when present.

import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import fs from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'
import { deadFake, flag, makeClient, quietLogger, startFake } from './helpers.mjs'

const require = createRequire(import.meta.url)
const {
    CheckgateStartupTimeoutError, createRequestFlags, createTestClient, normalizeAttributes, normalizeContext,
    startCheckgate, stopCheckgate,
} = require('@checkgate/node-integrations')
const { CheckgateCore } = require('@checkgate/node/native-binding.js')

test('normalizeAttributes keeps strings, stringifies numbers / bigints / booleans, drops the rest', () => {
    const input = {
        plan: 'pro', seats: 30, ratio: 0.5, negative: -1, big: 10n, beta: true, off: false,
        nan: NaN, inf: Infinity, nil: null, undef: undefined, obj: { a: 1 }, arr: ['x'], date: new Date(0),
        sym: Symbol('s'), fn: () => 1,
    }
    assert.deepEqual(normalizeAttributes(input), {
        plan: 'pro', seats: '30', ratio: '0.5', negative: '-1', big: '10', beta: 'true', off: 'false',
    })
    assert.deepEqual(normalizeAttributes(null), {})
    assert.deepEqual(normalizeAttributes('plan=pro'), {})
    // `__proto__` as an own key (JSON.parse makes those) cannot pollute the result.
    const hostile = JSON.parse('{"__proto__": {"polluted": "yes"}, "plan": "pro"}')
    const out = normalizeAttributes(hostile)
    assert.deepEqual(out, { plan: 'pro' })
    assert.equal(Object.getPrototypeOf(out), Object.prototype)
    // A throwing getter drops that attribute, not the request.
    const trap = { plan: 'pro', get boom() { throw new Error('getter') } }
    assert.deepEqual(normalizeAttributes(trap), { plan: 'pro' })
})

test('normalizeContext: object, bare key, nothing', () => {
    assert.deepEqual(normalizeContext({ key: 'u1', attributes: { n: 1 } }), { key: 'u1', attributes: { n: '1' } })
    assert.deepEqual(normalizeContext('u2'), { key: 'u2', attributes: {} })
    assert.deepEqual(normalizeContext(42), { key: '42', attributes: {} })
    assert.deepEqual(normalizeContext(null), { key: 'anonymous', attributes: {} })
    assert.deepEqual(normalizeContext({ key: '' }, 'guest'), { key: 'guest', attributes: {} })
    assert.equal(Object.isFrozen(normalizeContext({ key: 'u' }).attributes), true)
})

test('the native core throws on a non-string referenced attribute; the evaluator coerces instead', () => {
    const core = new CheckgateCore()
    core.upsertFlagV2(JSON.stringify(flag('big-team')))
    // What an app calling the SDK directly with a number hits.
    assert.throws(() => core.isEnabled('big-team', 'u', { seats: 30 }), /String/)
    const client = { isReady: () => true, isEnabled: (f, k, a) => core.isEnabled(f, k, a), getVariant: (f, k, a) => JSON.parse(core.getVariant(f, k, a)) }
    const errors = []
    const flags = createRequestFlags(client, () => ({ key: 'u', attributes: { seats: 30 } }), { onError: (e) => errors.push(e) })
    assert.equal(flags.isEnabled('big-team'), true)
    const few = createRequestFlags(client, () => ({ key: 'u', attributes: { seats: 3 } }))
    assert.equal(few.isEnabled('big-team'), false)
    // An object-valued attribute is dropped: the rule sees no attribute.
    const odd = createRequestFlags(client, () => ({ key: 'u', attributes: { seats: { n: 30 } } }), { onError: (e) => errors.push(e) })
    assert.equal(odd.isEnabled('big-team'), false)
    assert.deepEqual(errors, [])
})

test('the evaluator never throws: bad flag keys and throwing clients return defaults', () => {
    const errors = []
    const throwing = {
        isReady: () => true,
        isEnabled() { throw new Error('core exploded') },
        getVariant() { throw new Error('core exploded') },
        track() { throw new Error('track exploded') },
    }
    const flags = createRequestFlags(throwing, () => 'u', { onError: (err, info) => errors.push([err.message, info]) })
    assert.equal(flags.isEnabled('x'), false)
    assert.equal(flags.getVariant('x'), null)
    assert.equal(flags.getValue('x', 'dflt'), 'dflt')
    flags.track('goal')
    assert.equal(errors.length, 4)
    // An onError that throws is swallowed too.
    const loud = createRequestFlags(throwing, () => 'u', { onError: () => { throw new Error('handler') } })
    assert.equal(loud.isEnabled('x'), false)
})

test('the context is resolved once per request, on first use', () => {
    let calls = 0
    const flags = createRequestFlags(createTestClient({ a: true }), () => { calls++; return 'u' })
    assert.equal(calls, 0)
    flags.isEnabled('a')
    flags.getValue('a')
    void flags.context
    assert.equal(calls, 1)
})

test('startCheckgate validates its options', async () => {
    const client = createTestClient()
    await assert.rejects(startCheckgate(client, { onTimeout: 'maybe' }), /onTimeout must be/)
    await assert.rejects(startCheckgate(client, { readyTimeoutMs: -1 }), /readyTimeoutMs/)
    await assert.rejects(startCheckgate(client, { readyTimeoutMs: Infinity }), /readyTimeoutMs/)
    await assert.rejects(startCheckgate({}), /needs a CheckgateClient/)
    assert.deepEqual(await startCheckgate(client), { ready: true, timedOut: false })
})

test('startCheckgate with the real client: continue and fail timeouts', async () => {
    const dead = await deadFake()
    const cont = makeClient(dead.url)
    assert.deepEqual(await startCheckgate(cont, { readyTimeoutMs: 100, logger: quietLogger }), { ready: false, timedOut: true })
    assert.notEqual(cont.getStatus().state, 'closed', "'continue' keeps the client syncing")
    stopCheckgate(cont)

    const fail = makeClient(dead.url)
    await assert.rejects(startCheckgate(fail, { readyTimeoutMs: 100, onTimeout: 'fail' }), (err) => {
        assert.ok(err instanceof CheckgateStartupTimeoutError)
        assert.equal(err.timeoutMs, 100)
        return true
    })
    assert.equal(fail.getStatus().state, 'closed')
})

test('stopCheckgate disconnects once until started again', async () => {
    const client = createTestClient()
    await startCheckgate(client)
    assert.equal(stopCheckgate(client), true)
    assert.equal(stopCheckgate(client), false)
    assert.equal(client.disconnectCalls, 1)
    await startCheckgate(client)
    assert.equal(stopCheckgate(client), true)
    assert.equal(client.disconnectCalls, 2)
})

test('createTestClient semantics', () => {
    const client = createTestClient({ on: true, off: false, color: 'gold', nothing: null, perUser: ({ key }) => key === 'a' })
    assert.equal(client.isEnabled('on', 'u'), true)
    assert.equal(client.isEnabled('off', 'u'), false)
    assert.equal(client.isEnabled('color', 'u'), true)
    assert.equal(client.isEnabled('nothing', 'u'), false)
    assert.equal(client.isEnabled('missing', 'u'), false)
    assert.deepEqual(client.getVariant('color', 'u'), { enabled: true, value: 'gold' })
    assert.equal(client.getVariant('missing', 'u'), null)
    assert.equal(client.getValue('missing', 'u', {}, 'd'), 'd')
    assert.equal(client.isEnabled('perUser', 'a'), true)
    assert.equal(client.isEnabled('perUser', 'b'), false)
    client.setFlag('on', undefined)
    assert.equal(client.getVariant('on', 'u'), null)
})

test('a test client keeps the native addon unloaded (forTesting needs no build)', () => {
    const here = fileURLToPath(new URL('.', import.meta.url))
    const script = `
        require('reflect-metadata')
        const { CheckgateModule } = require('@checkgate/node-integrations/nestjs')
        require('@checkgate/node-integrations/express')
        require('@checkgate/node-integrations/fastify')
        CheckgateModule.forTesting({ a: true })
        const loaded = Object.keys(require.cache).filter((p) => /native-binding|\\.node$/.test(p))
        process.stdout.write(JSON.stringify(loaded))`
    const out = execFileSync(process.execPath, ['-e', script], { cwd: here })
    assert.deepEqual(JSON.parse(out.toString()), [])
})

// The cross-SDK evaluation vectors (sdks/conformance/vectors/evaluation.json),
// evaluated through the request evaluator over a real client and stream.
const vectorsPath = fileURLToPath(new URL('../../conformance/vectors/evaluation.json', import.meta.url))
const vectors = fs.existsSync(vectorsPath) ? JSON.parse(fs.readFileSync(vectorsPath, 'utf8')) : null
test('shared evaluation vectors', { skip: vectors ? false : 'sdks/conformance/vectors/evaluation.json is not on this branch' }, async () => {
    const fake = await startFake({ flags: vectors.flags })
    const client = makeClient(fake.url)
    await client.connect()
    try {
        for (const c of vectors.cases) {
            const flags = createRequestFlags(client, () => ({ key: c.user, attributes: c.attributes }))
            if (c.expect === null) {
                assert.equal(flags.getVariant(c.flag), null, c.name)
                continue
            }
            assert.equal(flags.isEnabled(c.flag), c.expect.enabled, `${c.name}: enabled`)
            assert.deepEqual(flags.getValue(c.flag), c.expect.value, `${c.name}: value`)
        }
    } finally {
        client.disconnect()
        await fake.stop()
    }
})

test('ESM importers see every named export of every entry point', async () => {
    const expected = {
        '@checkgate/node-integrations': ['CheckgateStartupTimeoutError', 'RequestFlags', 'createRequestFlags', 'createTestClient',
            'defaultContext', 'normalizeAttributes', 'normalizeContext', 'startCheckgate', 'stopCheckgate'],
        '@checkgate/node-integrations/express': ['checkgateMiddleware', 'listenWithCheckgate', 'startCheckgate', 'stopCheckgate'],
        '@checkgate/node-integrations/fastify': ['checkgatePlugin', 'default', 'startCheckgate', 'stopCheckgate'],
        '@checkgate/node-integrations/nestjs': ['CHECKGATE_CLIENT', 'CHECKGATE_OPTIONS', 'Checkgate', 'CheckgateFlagGuard',
            'CheckgateModule', 'CheckgateRequestPipe', 'CheckgateService', 'REQUIRE_FLAG_METADATA', 'RequireFlag',
            'createTestClient', 'startCheckgate', 'stopCheckgate'],
    }
    for (const [specifier, names] of Object.entries(expected)) {
        const ns = await import(specifier)
        for (const name of names) assert.notEqual(ns[name], undefined, `${specifier} exports ${name}`)
    }
})
