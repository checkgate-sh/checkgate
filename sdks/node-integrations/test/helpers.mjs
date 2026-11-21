// Shared fixtures for the integration tests: the conformance fake server
// (sdks/conformance/fault-server.mjs) as the Checkgate server, the REAL
// @checkgate/node client with its native addon, and one suite of behaviour
// checks that every framework adapter must pass.

import assert from 'node:assert/strict'
import http from 'node:http'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { setTimeout as sleep } from 'node:timers/promises'
import { FaultServer } from '../../conformance/fault-server.mjs'

const require = createRequire(import.meta.url)

let sdk
try {
    sdk = require('@checkgate/node')
} catch (err) {
    throw new Error('These tests need the real @checkgate/node with its native addon: ' +
        'run `npm ci && npm run build:debug` in sdks/nodejs, then `npm ci` here.\n' + err.message)
}
export const { CheckgateClient } = sdk

export const SDK_KEY = 'sk_integrations_test'

// The SDK logs every (re)connect; keep the output readable but keep what an
// assertion may want to look at.
export const logs = []
for (const level of ['log', 'warn', 'error']) {
    console[level] = (...args) => logs.push({ level, text: args.map(String).join(' ') })
}
export const quietLogger = { warn: (...args) => logs.push({ level: 'warn', text: args.join(' ') }) }

// Every network call the SDK makes goes through the global fetch (the stream
// via eventsource's fetch hook, polls, analytics). Test traffic to the app
// uses node:http, so this counts only Checkgate traffic.
export const sdkFetches = []
const realFetch = globalThis.fetch
globalThis.fetch = (url, init) => {
    sdkFetches.push(String(url))
    return realFetch(url, init)
}

// -- flags ------------------------------------------------------------------

const base = { rollout_percentage: null, description: null, rules: [] }
export const FLAGS = [
    // On only for plan=pro (rules only: rollout 0 serves nobody else).
    { ...base, key: 'pro-feature', is_enabled: true, rollout_percentage: 0,
        rules: [{ attribute: 'plan', operator: 'equals', values: ['pro'] }] },
    // A numeric rule: `seats` arrives as a number and must be coerced, not thrown on.
    { ...base, key: 'big-team', is_enabled: true, rollout_percentage: 0,
        rules: [{ attribute: 'seats', operator: 'greater_than', values: ['10'] }] },
    // A boolean attribute, coerced to "true" / "false".
    { ...base, key: 'beta-only', is_enabled: true, rollout_percentage: 0,
        rules: [{ attribute: 'beta', operator: 'equals', values: ['true'] }] },
    // Per-user hashing: half of all users.
    { ...base, key: 'half-rollout', is_enabled: true, rollout_percentage: 50 },
    // A string flag with a per-rule variant.
    { ...base, key: 'banner-color', is_enabled: true, flag_type: 'string', default_value: 'blue',
        disabled_value: 'grey', rules: [{ attribute: 'plan', operator: 'equals', values: ['pro'], variant: 'gold' }] },
]
export const flag = (key) => FLAGS.find((f) => f.key === key)

// -- the fake Checkgate server -----------------------------------------------

export async function startFake({ flags = FLAGS, ...opts } = {}) {
    const server = new FaultServer({ sdkKey: SDK_KEY, heartbeatMs: 100, ...opts })
    for (const f of flags) server.upsert(f)
    await server.start()
    return server
}

/** A URL where nothing listens (and the FaultServer that can be revived there). */
export async function deadFake(opts) {
    const server = await startFake(opts)
    const port = await killFake(server)
    return { server, port, url: server.url }
}

/** Takes the fake server down (every stream cut, port closed); returns its port. */
export async function killFake(server) {
    const port = Number(new URL(server.url).port)
    await server.stop()
    return port
}

/** Brings a killed fake server back on the same port, with the state it had. */
export async function reviveFake(server, port) {
    await new Promise((resolve, reject) => {
        server.server.once('error', reject)
        server.server.listen(port, '127.0.0.1', () => {
            server.server.off('error', reject)
            resolve()
        })
    })
}

export function makeClient(serverUrl, overrides = {}) {
    return new CheckgateClient({
        serverUrl,
        sdkKey: SDK_KEY,
        reportImpressions: false,
        // Production rules, test-speed constants (as sdks/conformance).
        reconnectDelayMs: 20,
        maxReconnectDelayMs: 200,
        connectTimeoutMs: 500,
        pollFallbackThreshold: 3,
        pollIntervalMs: 100,
        ...overrides,
    })
}

/** Wraps client.disconnect() to count calls. */
export function countDisconnects(client) {
    const counter = { calls: 0 }
    const original = client.disconnect.bind(client)
    client.disconnect = () => {
        counter.calls++
        original()
    }
    return counter
}

/** A storage adapter backed by a Map (the offline cache). */
export function memoryStorage() {
    const map = new Map()
    return { map, getItem: (k) => (map.has(k) ? map.get(k) : null), setItem: (k, v) => void map.set(k, v) }
}

export async function waitFor(predicate, { timeoutMs = 5000, label = 'condition' } = {}) {
    const deadline = Date.now() + timeoutMs
    for (;;) {
        if (await predicate()) return
        if (Date.now() > deadline) throw new Error(`timed out after ${timeoutMs} ms waiting for ${label}`)
        await sleep(20)
    }
}

// -- the app's view: one route, same answers in every framework -------------

/** The context extractor every test app uses: user and attributes from headers. */
export function contextFromHeaders(headers) {
    const key = headers['x-user']
    if (!key) return null
    return {
        key,
        attributes: {
            plan: headers['x-plan'],
            seats: headers['x-seats'] === undefined ? undefined : Number(headers['x-seats']),
            beta: headers['x-beta'] === undefined ? undefined : headers['x-beta'] === '1',
            // Not coercible: dropped, never sent to the core.
            meta: { nested: true },
        },
    }
}

/**
 * What every test app's `GET /check` answers, from the request's flags.
 * Waits a random few ms before evaluating so concurrent requests interleave
 * between "flags object created" and "flags evaluated".
 */
export async function checkResponse(flags) {
    await sleep(Math.random() * 5)
    const context = flags.context
    await sleep(Math.random() * 3)
    return {
        key: context.key,
        attributes: context.attributes,
        pro: flags.isEnabled('pro-feature'),
        bigTeam: flags.isEnabled('big-team'),
        beta: flags.isEnabled('beta-only'),
        half: flags.isEnabled('half-rollout'),
        color: flags.getValue('banner-color', 'none'),
        variant: flags.getVariant('banner-color'),
        missing: flags.getValue('no-such-flag', 'fallback'),
    }
}

/** The user and headers of test request `i`. */
export function userFor(i) {
    const plan = i % 3 === 0 ? 'pro' : 'free'
    const seats = (i % 4) * 7 // 0, 7, 14, 21
    const beta = i % 5 === 0
    return {
        key: `user-${i}`,
        plan,
        seats,
        beta,
        headers: { 'x-user': `user-${i}`, 'x-plan': plan, 'x-seats': String(seats), 'x-beta': beta ? '1' : '0' },
    }
}

const agent = new http.Agent({ keepAlive: true, maxSockets: 64 })

/** GET `path` on the app over real HTTP; resolves { status, body }. */
export function get(baseUrl, path, headers = {}) {
    return new Promise((resolve, reject) => {
        const req = http.get(new URL(path, baseUrl), { agent, headers }, (res) => {
            let data = ''
            res.setEncoding('utf8')
            res.on('data', (chunk) => (data += chunk))
            res.on('end', () => {
                let body = data
                try {
                    body = JSON.parse(data)
                } catch {
                    // not JSON (an error page)
                }
                resolve({ status: res.statusCode, body })
            })
        })
        req.on('error', reject)
    })
}

/** Asserts `/check` for user `u` matches what that user must get from `client`. */
function assertUserResponse(client, u, body) {
    assert.equal(body.key, u.key, 'context leaked from another request')
    assert.deepEqual(body.attributes, {
        plan: u.plan,
        seats: String(u.seats),
        beta: u.beta ? 'true' : 'false',
    })
    assert.equal(body.pro, u.plan === 'pro', `${u.key} pro-feature`)
    assert.equal(body.bigTeam, u.seats > 10, `${u.key} big-team`)
    assert.equal(body.beta, u.beta, `${u.key} beta-only`)
    // Rollout hashing of this user's key, evaluated directly on the client.
    assert.equal(body.half, client.isEnabled('half-rollout', u.key, {}), `${u.key} half-rollout`)
    assert.equal(body.color, u.plan === 'pro' ? 'gold' : 'blue', `${u.key} banner-color`)
    assert.deepEqual(body.variant, { enabled: true, value: u.plan === 'pro' ? 'gold' : 'blue' })
    assert.equal(body.missing, 'fallback')
}

// -- the suite ----------------------------------------------------------------

/**
 * The behaviour every adapter must have. `adapter.start({ client, readyTimeoutMs, onTimeout })`
 * starts an app whose `GET /check` answers checkResponse(request flags), and
 * resolves `{ url, close() }` once it is listening (rejecting if startup fails).
 */
export function frameworkSuite(name, adapter) {
    const opts = { concurrency: false }

    test(`[${name}] concurrent requests each evaluate for their own user`, opts, async () => {
        const fake = await startFake()
        const client = makeClient(fake.url)
        const app = await adapter.start({ client, readyTimeoutMs: 3000, onTimeout: 'fail' })
        try {
            assert.equal(client.isReady(), true)
            const users = Array.from({ length: 400 }, (_, i) => userFor(i))
            const responses = await Promise.all(users.map((u) => get(app.url, '/check', u.headers)))
            responses.forEach((res, i) => {
                assert.equal(res.status, 200)
                assertUserResponse(client, users[i], res.body)
            })
            // Sanity: the population really is mixed.
            assert.ok(responses.some((r) => r.body.half) && responses.some((r) => !r.body.half))
            // An anonymous request gets the default key and no attributes.
            const anon = await get(app.url, '/check')
            assert.equal(anon.body.key, 'anonymous')
            assert.deepEqual(anon.body.attributes, {})
            assert.equal(fake.streams.size, 1, 'one stream for the whole app')
        } finally {
            await app.close()
            await fake.stop()
        }
    })

    test(`[${name}] startup: server up -> ready`, opts, async () => {
        const fake = await startFake()
        const client = makeClient(fake.url)
        const started = Date.now()
        const app = await adapter.start({ client, readyTimeoutMs: 3000, onTimeout: 'fail' })
        try {
            assert.equal(client.isReady(), true)
            assert.ok(Date.now() - started < 3000)
            const res = await get(app.url, '/check', userFor(0).headers)
            assert.equal(res.body.pro, true)
        } finally {
            await app.close()
            await fake.stop()
        }
    })

    test(`[${name}] startup: server down, onTimeout 'continue' starts on time, serves defaults, then syncs`, opts, async () => {
        const dead = await deadFake()
        const client = makeClient(dead.url)
        const started = Date.now()
        const app = await adapter.start({ client, readyTimeoutMs: 300, onTimeout: 'continue' })
        try {
            const elapsed = Date.now() - started
            assert.ok(elapsed >= 280 && elapsed < 3000, `started after ${elapsed} ms`)
            assert.equal(client.isReady(), false)
            const before = logs.length
            const res = await get(app.url, '/check', userFor(0).headers)
            assert.equal(res.status, 200)
            assert.equal(res.body.pro, false)
            assert.equal(res.body.color, 'none')
            assert.equal(res.body.variant, null)
            assert.equal(res.body.missing, 'fallback')
            // No per-request "called before connect() resolved" warning.
            assert.equal(logs.slice(before).filter((l) => /before connect\(\) resolved/.test(l.text)).length, 0)
            // The client kept trying in the background: bring the server up.
            await reviveFake(dead.server, dead.port)
            await waitFor(async () => (await get(app.url, '/check', userFor(0).headers)).body.pro === true,
                { label: 'flags after the server came up' })
            assert.equal(client.isReady(), true)
        } finally {
            await app.close()
            await dead.server.stop()
        }
    })

    test(`[${name}] startup: server down, onTimeout 'continue' serves the offline cache`, opts, async () => {
        const fake = await startFake()
        const storage = memoryStorage()
        const first = makeClient(fake.url, { storage })
        await first.connect()
        await waitFor(() => storage.map.size > 0, { label: 'cache persisted' })
        first.disconnect()
        await killFake(fake)

        const client = makeClient(fake.url, { storage })
        const app = await adapter.start({ client, readyTimeoutMs: 300, onTimeout: 'continue' })
        try {
            assert.equal(client.isReady(), false)
            const responses = await Promise.all([0, 1, 2, 3].map((i) => get(app.url, '/check', userFor(i).headers)))
            responses.forEach((res, i) => assertUserResponse(client, userFor(i), res.body))
        } finally {
            await app.close()
        }
    })

    test(`[${name}] startup: server down, onTimeout 'fail' rejects and stops retrying`, opts, async () => {
        const dead = await deadFake()
        const client = makeClient(dead.url)
        const disconnects = countDisconnects(client)
        const started = Date.now()
        await assert.rejects(adapter.start({ client, readyTimeoutMs: 300, onTimeout: 'fail' }),
            (err) => err.name === 'CheckgateStartupTimeoutError')
        const elapsed = Date.now() - started
        assert.ok(elapsed >= 280 && elapsed < 3000, `failed after ${elapsed} ms`)
        assert.equal(disconnects.calls, 1)
        // Once the server is back, a stopped client does not connect to it.
        await reviveFake(dead.server, dead.port)
        await sleep(400)
        assert.equal(dead.server.counters.streamRequests, 0)
        await dead.server.stop()
    })

    test(`[${name}] startup: a rejected SDK key fails startup even with onTimeout 'continue'`, opts, async () => {
        const fake = await startFake()
        fake.setAcceptedKey('sk_some_other_key')
        const client = makeClient(fake.url)
        await assert.rejects(adapter.start({ client, readyTimeoutMs: 3000, onTimeout: 'continue' }),
            (err) => err.name === 'CheckgateAuthError' && err.status === 401)
        assert.ok(fake.counters.unauthorized >= 1)
        // Not retried: a rejected key does not become valid by asking again.
        const requests = fake.counters.streamRequests + fake.counters.snapshotRequests
        await sleep(400)
        assert.equal(fake.counters.streamRequests + fake.counters.snapshotRequests, requests)
        await fake.stop()
    })

    test(`[${name}] live change: next requests see it, with no per-request network call`, opts, async () => {
        // The fake server shares this process's event loop: a slow heartbeat
        // keeps a burst of requests from delaying it past the 3-heartbeat
        // idle deadline (which would reconnect, legitimately, and muddle the count).
        const fake = await startFake({ heartbeatMs: 2000 })
        const client = makeClient(fake.url)
        const app = await adapter.start({ client, readyTimeoutMs: 3000, onTimeout: 'fail' })
        try {
            const free = userFor(1)
            assert.equal((await get(app.url, '/check', free.headers)).body.pro, false)
            // Turn pro-feature on for the free plan too.
            fake.upsert({ ...flag('pro-feature'), rules: [{ attribute: 'plan', operator: 'equals', values: ['pro', 'free'] }] })
            await waitFor(async () => (await get(app.url, '/check', free.headers)).body.pro === true,
                { label: 'the change to reach requests' })

            const counters = { ...fake.counters }
            const fetches = sdkFetches.length
            const users = Array.from({ length: 200 }, (_, i) => userFor(i))
            const responses = await Promise.all(users.map((u) => get(app.url, '/check', u.headers)))
            responses.forEach((res) => assert.equal(res.body.pro, true))
            assert.equal(fake.counters.streamRequests, counters.streamRequests, 'no stream request per request')
            assert.equal(fake.counters.snapshotRequests, counters.snapshotRequests, 'no snapshot request per request')
            assert.equal(sdkFetches.length, fetches, 'the SDK made no network call while serving requests')
            assert.equal(fake.streams.size, 1)
        } finally {
            await app.close()
            await fake.stop()
        }
    })

    test(`[${name}] offline: server killed mid-run keeps answering from held flags, converges on restart`, opts, async () => {
        const fake = await startFake()
        const client = makeClient(fake.url)
        const app = await adapter.start({ client, readyTimeoutMs: 3000, onTimeout: 'fail' })
        try {
            const users = Array.from({ length: 60 }, (_, i) => userFor(i))
            const port = await killFake(fake)
            const during = await Promise.all(users.map((u) => get(app.url, '/check', u.headers)))
            during.forEach((res, i) => assertUserResponse(client, users[i], res.body))
            // Changed while the app could not hear about it.
            fake.upsert({ ...flag('banner-color'), default_value: 'green' })
            await sleep(300)
            const stillOld = await get(app.url, '/check', userFor(1).headers)
            assert.equal(stillOld.body.color, 'blue')

            await reviveFake(fake, port)
            await waitFor(async () => (await get(app.url, '/check', userFor(1).headers)).body.color === 'green',
                { label: 'convergence after restart' })
            await waitFor(() => fake.streams.size === 1, { label: 'one stream after recovery' })
            // Everything else unchanged.
            const after = await get(app.url, '/check', userFor(3).headers)
            assert.equal(after.body.pro, true)
            assert.equal(after.body.color, 'gold')
        } finally {
            await app.close()
            await fake.stop()
        }
    })

    test(`[${name}] shutdown: close disconnects exactly once and leaves no stream open`, opts, async () => {
        const fake = await startFake()
        const client = makeClient(fake.url)
        const disconnects = countDisconnects(client)
        const app = await adapter.start({ client, readyTimeoutMs: 3000, onTimeout: 'fail' })
        assert.equal(fake.streams.size, 1)
        await app.close()
        await app.close() // a second close (signal + manual) must not disconnect again
        assert.equal(disconnects.calls, 1)
        assert.equal(client.getStatus().state, 'closed')
        await waitFor(() => fake.streams.size === 0, { label: 'the stream to close' })
        const requests = fake.counters.streamRequests
        await sleep(400)
        assert.equal(fake.counters.streamRequests, requests, 'no reconnect after shutdown')
        await fake.stop()
    })
}

export { sleep }
