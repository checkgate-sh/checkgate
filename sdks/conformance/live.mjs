// Live end-to-end suite: each SDK's real client, transport and engine
// (adapters.mjs) against a running Checkgate server, through an HTTP proxy
// that can sever, stall, or refuse only `/stream`.
//
//   node sdks/conformance/live.mjs [node|web|react-native|flutter|all]
//
// Env: CHECKGATE_URL          the server (default http://127.0.0.1:3000); run it
//                             with SSE_HEARTBEAT_INTERVAL_SECS=1
//      CHECKGATE_SETUP_TOKEN  first-run setup of a fresh database, or
//      CHECKGATE_E2E_SESSION  a JSON file to reuse (written after setup)
//      CHECKGATE_FLUTTER_LIB  the host build of the Flutter engine
//
// scripts/sdk-live-e2e.sh starts a server on a fresh database and runs this.
// Prints one JSON line per check and exits non-zero if any failed.

import assert from 'node:assert';
import fs from 'node:fs';
import http from 'node:http';
import { ADAPTERS } from './adapters.mjs';

const BASE = process.env.CHECKGATE_URL || 'http://127.0.0.1:3000';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const TIMING = {
    reconnectDelayMs: 100, maxReconnectDelayMs: 1000, pollFallbackThreshold: 2,
    pollIntervalMs: 500, connectTimeoutMs: 3000,
};
/** With a 1 s server heartbeat: idle deadline 3 s, plus backoff and bootstrap. */
const RECOVER_MS = 8000;

async function api(method, path, { token, cookie, body, setupToken } = {}) {
    const res = await fetch(`${BASE}${path}`, {
        method,
        headers: {
            'Content-Type': 'application/json',
            'X-Checkgate-Request': 'true',
            ...(setupToken ? { 'X-Checkgate-Setup-Token': setupToken } : {}),
            ...(token ? { Authorization: `Bearer ${token}` } : {}),
            ...(cookie ? { Cookie: cookie } : {}),
        },
        body: body ? JSON.stringify(body) : undefined,
    });
    const text = await res.text();
    let json = null;
    try { json = text ? JSON.parse(text) : null; } catch { json = text; }
    return { status: res.status, json };
}

async function session() {
    const file = process.env.CHECKGATE_E2E_SESSION;
    if (file && fs.existsSync(file)) return JSON.parse(fs.readFileSync(file, 'utf8'));
    const setupToken = process.env.CHECKGATE_SETUP_TOKEN || '';
    const key = await api('GET', '/api/setup/key', { setupToken });
    assert.equal(key.status, 200, 'GET /api/setup/key (needs a fresh database)');
    const res = await fetch(`${BASE}/api/setup/complete`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-Checkgate-Request': 'true', 'X-Checkgate-Setup-Token': setupToken },
        body: JSON.stringify({ workspace_name: 'Live', project_name: 'P', name: 'A', email: 'a@live.test', password: 'supersecret1' }),
    });
    assert.equal(res.status, 200, 'POST /api/setup/complete');
    const cookie = res.headers.getSetCookie().map((c) => c.split(';')[0]).join('; ');
    const token = await api('POST', '/api/tokens', { cookie, body: { name: 'live', scope: 'read_write' } });
    assert.equal(token.status, 200, 'POST /api/tokens');
    const projects = await api('GET', '/api/projects', { token: token.json.token });
    const list = Array.isArray(projects.json) ? projects.json : (projects.json.projects || projects.json.data || []);
    const s = { sdkKey: key.json.key, env: key.json.environment_id, admin: token.json.token, project: list[0] && list[0].id };
    if (file) fs.writeFileSync(file, JSON.stringify(s));
    return s;
}

/** An HTTP proxy in front of the server, with faults and per-key request counts. */
async function startProxy() {
    const target = new URL(BASE);
    const state = { mode: 'pass', blockStream: false, open: new Set(), requests: new Map() };
    const server = http.createServer((req, res) => {
        const auth = req.headers.authorization || new URL(req.url, 'http://x').searchParams.get('sdk_key') || '';
        const key = auth.replace(/^Bearer /, '');
        state.requests.set(key, (state.requests.get(key) || 0) + 1);
        if (state.blockStream && req.url.startsWith('/stream')) {
            res.writeHead(503, { 'Retry-After': '1' }).end();
            return;
        }
        const up = http.request({ host: target.hostname, port: target.port, path: req.url, method: req.method, headers: req.headers }, (upRes) => {
            res.writeHead(upRes.statusCode, upRes.headers);
            const conn = { req, res, upRes, stalled: false };
            state.open.add(conn);
            upRes.on('data', (chunk) => { if (!conn.stalled) res.write(chunk); });
            upRes.on('end', () => { state.open.delete(conn); res.end(); });
            upRes.on('error', () => { state.open.delete(conn); res.destroy(); });
            res.on('close', () => { state.open.delete(conn); upRes.destroy(); });
        });
        up.on('error', () => res.destroy());
        req.pipe(up);
    });
    await new Promise((r) => server.listen(0, '127.0.0.1', r));
    return {
        url: `http://127.0.0.1:${server.address().port}`,
        sever() { for (const c of state.open) { c.res.destroy(); c.upRes.destroy(); } state.open.clear(); },
        /** Open streams go silent with their sockets left open (a dropped NAT mapping). */
        stall() { for (const c of state.open) c.stalled = true; },
        set blockStream(v) { state.blockStream = v; },
        streams: () => [...state.open].filter((c) => c.req.url.startsWith('/stream')).length,
        requestsFor: (key) => state.requests.get(key) || 0,
        close: () => new Promise((r) => { server.closeAllConnections(); server.close(r); }),
    };
}

async function waitFor(what, ms, probe) {
    const start = Date.now();
    for (;;) {
        if (await probe()) return Date.now() - start;
        if (Date.now() - start > ms) throw new Error(`timed out after ${ms} ms: ${what}`);
        await sleep(50);
    }
}

async function runSdk(name, s) {
    const adapter = await ADAPTERS[name]();
    const proxy = await startProxy();
    const p = `${name}-${Date.now().toString(36)}`;
    const flags = `/api/environments/${s.env}/flags`;
    const results = [];
    const clients = [];
    const check = async (step, fn) => {
        const t0 = Date.now();
        try {
            const detail = await fn();
            results.push({ sdk: name, engine: adapter.engine, step, ok: true, ms: Date.now() - t0, ...(detail || {}) });
        } catch (err) {
            results.push({ sdk: name, engine: adapter.engine, step, ok: false, error: String(err && err.message || err) });
        }
        // Not console.log: the adapters silence the SDKs' console output.
        process.stdout.write(JSON.stringify(results[results.length - 1]) + '\n');
    };
    const create = (key, body) => api('POST', flags, { token: s.admin, body: { key, flag_type: 'boolean', rules: [], ...body } });
    const patch = (key, body) => api('PATCH', `${flags}/${key}`, { token: s.admin, body });
    const enabled = async (c, key, attrs = {}) => (await adapter.evaluate(c, key, 'user-1', attrs)) === true;

    let client;
    try {
        await check('bootstrap', async () => {
            assert.equal((await create(`${p}-boot`, { is_enabled: true, rollout_percentage: 100 })).status, 200);
            client = await adapter.create(proxy.url, s.sdkKey, TIMING);
            clients.push(client);
            await adapter.connect(client);
            assert.equal(await enabled(client, `${p}-boot`), true, 'bootstrapped flag evaluates true');
            assert.equal((await adapter.status(client)).state, 'streaming');
        });

        await check('update delivery and targeting', async () => {
            const key = `${p}-live`;
            const t0 = Date.now();
            await create(key, { is_enabled: true, rollout_percentage: 0, rules: [{ attribute: 'plan', operator: 'equals', values: ['pro'] }] });
            await waitFor('created flag arrives', RECOVER_MS, async () => (await adapter.flags(client)).has(key));
            const created = Date.now() - t0;
            assert.equal(await enabled(client, key, { plan: 'pro' }), true, 'rule matches pro');
            assert.equal(await enabled(client, key, { plan: 'free' }), false, 'rollout 0 excludes free');
            const t1 = Date.now();
            await patch(key, { is_enabled: false });
            await waitFor('disable arrives', RECOVER_MS, async () => !(await enabled(client, key, { plan: 'pro' })));
            const patched = Date.now() - t1;
            const t2 = Date.now();
            assert.equal((await api('DELETE', `${flags}/${key}`, { token: s.admin })).status, 204);
            await waitFor('delete arrives', RECOVER_MS, async () => !(await adapter.flags(client)).has(key));
            return { createMs: created, patchMs: patched, deleteMs: Date.now() - t2 };
        });

        await check('reconnect after the link drops', async () => {
            proxy.sever();
            await patch(`${p}-boot`, { is_enabled: false });
            const ms = await waitFor('change made while down arrives', RECOVER_MS, async () => !(await enabled(client, `${p}-boot`)));
            return { recoverMs: ms };
        });

        await check('half-open stream is detected', async () => {
            proxy.stall();
            await patch(`${p}-boot`, { is_enabled: true });
            const ms = await waitFor('change arrives after the silent stream is replaced', RECOVER_MS, async () => enabled(client, `${p}-boot`));
            assert.equal((await adapter.status(client)).state, 'streaming');
            return { recoverMs: ms };
        });

        await check('poll fallback when only the stream is blocked', async () => {
            proxy.blockStream = true;
            proxy.sever();
            await waitFor('client polls', RECOVER_MS, async () => (await adapter.status(client)).state === 'polling');
            await patch(`${p}-boot`, { is_enabled: false });
            const ms = await waitFor('change arrives by polling', RECOVER_MS, async () => !(await enabled(client, `${p}-boot`)));
            proxy.blockStream = false;
            await waitFor('stream resumes', RECOVER_MS * 2, async () => (await adapter.status(client)).state === 'streaming');
            return { pollMs: ms };
        });

        await check('invalid key: connect() rejects and retries stop', async () => {
            const bad = await adapter.create(proxy.url, 'sdk_not_a_real_key', TIMING);
            clients.push(bad);
            const err = await adapter.connect(bad).then(() => null, (e) => e);
            assert.ok(err, 'connect() rejected');
            assert.match(String(err.name), /CheckgateAuth/);
            assert.equal((await adapter.status(bad)).state, 'auth_failed');
            const n = proxy.requestsFor('sdk_not_a_real_key');
            await sleep(2000);
            assert.equal(proxy.requestsFor('sdk_not_a_real_key'), n, 'no retry with a rejected key');
            return { requests: n };
        });

        await check('revoked key: terminal, last flags kept', async () => {
            assert.ok(s.project, 'project id from /api/projects');
            const made = await api('POST', `/api/projects/${s.project}/keys`, { token: s.admin, body: { name: `${p}-key`, environment_id: s.env } });
            assert.equal(made.status, 200, `create key: ${JSON.stringify(made.json)}`);
            const value = made.json.key || made.json.value;
            const revoked = await adapter.create(proxy.url, value, TIMING);
            clients.push(revoked);
            await adapter.connect(revoked);
            assert.equal(await enabled(revoked, `${p}-boot`), false);
            assert.equal((await api('DELETE', `/api/projects/${s.project}/keys/${made.json.id}`, { token: s.admin })).status < 300, true);
            // Open streams outlive a revocation (documented); the next attempt is refused.
            proxy.sever();
            const ms = await waitFor('auth_failed', RECOVER_MS, async () => (await adapter.status(revoked)).state === 'auth_failed');
            assert.equal((await adapter.flags(revoked)).has(`${p}-boot`), true, 'last known flags kept');
            const n = proxy.requestsFor(value);
            await sleep(2000);
            assert.equal(proxy.requestsFor(value), n, 'no retry after revocation');
            return { detectMs: ms };
        });

        await check('shutdown releases every stream', async () => {
            for (const c of clients.splice(0)) await adapter.close(c);
            await waitFor('no open streams', 3000, () => proxy.streams() === 0);
        });
    } finally {
        for (const c of clients) await adapter.close(c).catch(() => {});
        await proxy.close();
    }
    return results;
}

const which = process.argv[2] || 'all';
const s = await session();
if (which === 'all') {
    // One process per SDK: each installs its own EventSource and globals.
    const { spawnSync } = await import('node:child_process');
    const file = process.env.CHECKGATE_E2E_SESSION || `${process.env.TMPDIR || '/tmp'}/checkgate-live-session.json`;
    fs.writeFileSync(file, JSON.stringify(s));
    let failed = 0;
    for (const name of Object.keys(ADAPTERS)) {
        const r = spawnSync(process.execPath, [new URL(import.meta.url).pathname, name], {
            stdio: 'inherit', env: { ...process.env, CHECKGATE_E2E_SESSION: file },
        });
        if (r.status !== 0) failed++;
    }
    process.exit(failed ? 1 : 0);
}
const results = await runSdk(which, s);
if (results.length === 0) throw new Error('no checks ran');
process.exit(results.some((r) => !r.ok) ? 1 : 0);
