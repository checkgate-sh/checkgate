// Runs each example app (examples/*) as its own process against the fake
// Checkgate server: it must start, answer from flags, and on SIGTERM close
// its stream to the server and exit.

import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import { once } from 'node:events'
import { fileURLToPath } from 'node:url'
import { test } from 'node:test'
import { SDK_KEY, get, startFake, waitFor } from './helpers.mjs'

const root = fileURLToPath(new URL('..', import.meta.url))
const base = { rollout_percentage: null, description: null, rules: [] }
const flags = [
    { ...base, key: 'new-checkout', is_enabled: true, rollout_percentage: 0,
        rules: [{ attribute: 'plan', operator: 'equals', values: ['pro'] }] },
    { ...base, key: 'banner-color', is_enabled: true, flag_type: 'string', default_value: 'green' },
    { ...base, key: 'beta-page', is_enabled: true, rollout_percentage: 0,
        rules: [{ attribute: 'plan', operator: 'equals', values: ['pro'] }] },
]

async function runExample(entry, check) {
    const fake = await startFake({ flags })
    const child = spawn(process.execPath, [entry], {
        cwd: root,
        env: { ...process.env, CHECKGATE_URL: fake.url, CHECKGATE_SDK_KEY: SDK_KEY, PORT: '0' },
        stdio: ['ignore', 'pipe', 'pipe'],
    })
    let output = ''
    child.stdout.on('data', (d) => (output += d))
    child.stderr.on('data', (d) => (output += d))
    try {
        await waitFor(() => /listening on (http:\/\/\S+)/.test(output), { timeoutMs: 20000, label: `${entry} to listen` })
        const url = output.match(/listening on (http:\/\/\S+)/)[1].replace('[::1]', '127.0.0.1').replace('localhost', '127.0.0.1')
        assert.equal(fake.streams.size, 1)
        await check(url)
        child.kill('SIGTERM')
        const [code, signal] = await once(child, 'exit')
        // Express/Fastify exit 0; Nest re-raises the signal after its shutdown hooks.
        assert.ok(code === 0 || signal === 'SIGTERM', `exit ${code} ${signal}\n${output}`)
        await waitFor(() => fake.streams.size === 0, { label: 'the stream to close' })
    } finally {
        if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL')
        await fake.stop()
    }
}

async function checkHome(url) {
    const pro = await get(url, '/', { 'x-user-id': 'alice', 'x-plan': 'pro' })
    assert.deepEqual(pro.body, { user: 'alice', newCheckout: true, bannerColor: 'green' })
    const free = await get(url, '/', { 'x-user-id': 'bob' })
    assert.deepEqual(free.body, { user: 'bob', newCheckout: false, bannerColor: 'green' })
}

test('[example] express', async () => {
    await runExample('examples/express/server.mjs', checkHome)
})

test('[example] fastify', async () => {
    await runExample('examples/fastify/server.mjs', checkHome)
})

test('[example] nestjs (compiled with tsc)', async () => {
    execFileSync(process.execPath, [fileURLToPath(new URL('../node_modules/typescript/bin/tsc', import.meta.url)), '-p', 'examples/nestjs'], { cwd: root })
    await runExample('examples/nestjs/dist/main.js', async (url) => {
        await checkHome(url)
        assert.equal((await get(url, '/beta', { 'x-user-id': 'alice', 'x-plan': 'pro' })).status, 200)
        assert.equal((await get(url, '/beta', { 'x-user-id': 'bob' })).status, 404)
    })
})
