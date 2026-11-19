// Live server-restart check: one SDK's real client, transport and engine
// (adapters.mjs) against a server this script starts, kills (SIGKILL, as a
// crash) and starts again on the same port and database. While the server is
// down a flag cannot change; right after the restart it changes twice. The
// SDK must reconnect, end on the newest version, and never serve an older
// version than one it already served (descriptions count up).
//
//   CHECKGATE_SERVER_BIN=target/debug/server CHECKGATE_E2E_SESSION=session.json \
//   DATABASE_URL=… REDIS_URL=… node sdks/conformance/live-restart.mjs <sdk>
//
// The session file comes from live.mjs (scripts/sdk-live-e2e.sh runs both).
// Prints one JSON line and exits non-zero on failure.

import assert from 'node:assert';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { ADAPTERS } from './adapters.mjs';

const which = process.argv[2];
const BIN = process.env.CHECKGATE_SERVER_BIN;
const PORT = Number(process.env.RESTART_PORT || 3198);
const BASE = `http://127.0.0.1:${PORT}`;
const s = JSON.parse(fs.readFileSync(process.env.CHECKGATE_E2E_SESSION, 'utf8'));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const TIMING = {
    reconnectDelayMs: 100, maxReconnectDelayMs: 1000, pollFallbackThreshold: 2,
    pollIntervalMs: 500, connectTimeoutMs: 3000,
};

function startServer() {
    const env = {
        ...process.env, PORT: String(PORT), PUBLIC_DIR: fs.mkdtempSync(path.join(os.tmpdir(), 'cg-pub-')),
        SESSION_SECRET: 'live-e2e-0123456789-0123456789-0123456789-0123456789',
        COOKIE_SECURE: 'false', SSE_HEARTBEAT_INTERVAL_SECS: '1', RUST_LOG: 'warn',
        RATE_LIMIT_SDK_PER_MINUTE: '0', RATE_LIMIT_SSE_PER_MINUTE: '0', RATE_LIMIT_API_PER_MINUTE: '0',
    };
    return spawn(BIN, [], { env, stdio: ['ignore', 'ignore', 'inherit'] });
}

async function waitHealthy(ms) {
    const until = Date.now() + ms;
    while (Date.now() < until) {
        try {
            if ((await fetch(`${BASE}/health/ready`)).ok) return;
        } catch { /* not up yet */ }
        await sleep(100);
    }
    throw new Error('server did not become ready');
}

async function api(method, p, body) {
    const res = await fetch(`${BASE}${p}`, {
        method,
        headers: { 'Content-Type': 'application/json', 'X-Checkgate-Request': 'true', Authorization: `Bearer ${s.admin}` },
        body: body ? JSON.stringify(body) : undefined,
    });
    return res.status;
}

async function waitFor(what, ms, probe) {
    const start = Date.now();
    for (;;) {
        if (await probe()) return Date.now() - start;
        if (Date.now() - start > ms) throw new Error(`timed out after ${ms} ms: ${what}`);
        await sleep(25);
    }
}

const adapter = await ADAPTERS[which]();
let server = startServer();
const result = { sdk: which, engine: adapter.engine, step: 'server restart (SIGKILL) and recovery' };
let client;
try {
    await waitHealthy(30000);
    const key = `restart-${which}-${Date.now().toString(36)}`;
    const flags = `/api/environments/${s.env}/flags`;
    assert.equal(await api('POST', flags, { key, flag_type: 'boolean', is_enabled: true, rules: [], description: '1' }), 200);
    client = await adapter.create(BASE, s.sdkKey, TIMING);
    await adapter.connect(client);
    const desc = async () => {
        const f = (await adapter.flags(client)).get(key);
        return f ? Number(f.description) : 0;
    };
    await waitFor('flag at 1', 8000, async () => (await desc()) === 1);

    // Sample for the whole restart: the version served must never go back.
    let sampling = true;
    let highest = 0;
    const regressions = [];
    const sampler = (async () => {
        while (sampling) {
            const d = await desc();
            if (d < highest) regressions.push(`${highest} -> ${d}`);
            highest = Math.max(highest, d);
            await sleep(10);
        }
    })();

    server.kill('SIGKILL');
    await new Promise((r) => server.once('exit', r));
    const downAt = Date.now();
    await sleep(1500); // the SDK notices, backs off, possibly starts polling
    server = startServer();
    await waitHealthy(30000);
    const upAt = Date.now();
    assert.equal(await api('PATCH', `${flags}/${key}`, { description: '2' }), 200);
    assert.equal(await api('PATCH', `${flags}/${key}`, { description: '3', is_enabled: false }), 200);
    const recoverMs = await waitFor('flag at 3 after the restart', 15000, async () => (await desc()) === 3);
    await waitFor('streaming again', 15000, async () => (await adapter.status(client)).state === 'streaming');
    await sleep(500);
    sampling = false;
    await sampler;
    assert.deepEqual(regressions, [], 'went back to an older version');
    assert.equal(await adapter.evaluate(client, key, 'user-1'), false, 'evaluates the newest version');
    Object.assign(result, { ok: true, downMs: upAt - downAt, recoverAfterRestartMs: recoverMs });
} catch (err) {
    Object.assign(result, { ok: false, error: String(err && err.message || err) });
} finally {
    server.kill('SIGTERM');
    try {
        if (client) await adapter.close(client);
    } catch { /* closing a client of a stopped server */ }
}
process.stdout.write(JSON.stringify(result) + '\n');
process.exit(result.ok ? 0 : 1);
