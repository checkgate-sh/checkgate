// getHealth() against the conformance FaultServer: the diagnostics must say
// what actually happened on the wire. Real client, real eventsource; the
// engine is the native addon when built.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { FaultServer } from '../../conformance/fault-server.mjs';

const require = createRequire(import.meta.url);
let CheckgateClient;
try {
    ({ CheckgateClient } = require('../index.js'));
} catch {
    CheckgateClient = null;
}
const opts = { skip: CheckgateClient ? false : 'native addon not built', timeout: 30_000 };
for (const level of ['log', 'warn', 'error']) console[level] = () => {};

const FAST = {
    sdkKey: 'sk_conformance', reportImpressions: false, reconnectDelayMs: 20,
    maxReconnectDelayMs: 80, pollIntervalMs: 50, connectTimeoutMs: 500,
};
const flag = (key, on = true) => ({ key, is_enabled: on, rollout_percentage: null, rules: [] });
function until(predicate, ms = 5000) {
    const start = Date.now();
    return new Promise((resolve, reject) => {
        const tick = () => (predicate() ? resolve() : Date.now() - start > ms ? reject(new Error('timeout')) : setTimeout(tick, 10));
        tick();
    });
}

test('a fresh client reports nothing and contains no secrets', opts, () => {
    const c = new CheckgateClient({ serverUrl: 'http://127.0.0.1:9', ...FAST });
    const h = c.getHealth();
    assert.deepEqual(h, {
        schemaVersion: 1, state: 'idle', ready: false, source: 'none', flagCount: 0,
        lastSyncAt: null, configAgeMs: null, lastReceivedRevision: null, lastAppliedRevision: null,
        staleEventsIgnored: 0, reconnectAttempts: 0, reconnectCount: 0, polling: false,
        lastPollAt: null, lastError: null,
        offlineCache: { configured: false, hydrated: false, lastPersistAt: null },
    });
    assert.ok(!JSON.stringify(h).includes('sk_conformance'));
    c.disconnect();
});

test('streaming: revisions, stale events, reconnects and errors are observed', opts, async () => {
    const server = new FaultServer();
    const url = await server.start();
    server.upsert(flag('a'));
    server.upsert(flag('b'));
    server.upsert(flag('a', false)); // 'a' now has an older revision to replay
    const persisted = [];
    const c = new CheckgateClient({
        serverUrl: url, ...FAST, pollFallbackThreshold: Infinity,
        storage: { getItem: () => null, setItem: (k, v) => persisted.push(v) },
    });
    try {
        await c.connect();
        let h = c.getHealth();
        assert.equal(h.state, 'streaming');
        assert.equal(h.ready, true);
        assert.equal(h.source, 'server');
        assert.equal(h.flagCount, 2);
        assert.equal(h.lastAppliedRevision, 3);
        assert.equal(h.lastReceivedRevision, 3);
        assert.ok(h.configAgeMs >= 0 && h.configAgeMs < 2000);
        assert.equal(h.offlineCache.configured, true);
        await until(() => c.getHealth().offlineCache.lastPersistAt !== null);

        // An older revision replayed after the next ready is ignored and counted.
        server.staleAfterNextReady();
        server.severAll();
        await until(() => c.getHealth().reconnectCount >= 1 && c.getHealth().state === 'streaming');
        await until(() => c.getHealth().staleEventsIgnored >= 1);
        h = c.getHealth();
        assert.equal(h.lastAppliedRevision, 3, 'stale event not applied');
        assert.equal(h.reconnectAttempts, 0, 'reset by the successful reconnect');
        assert.equal(h.lastError.kind, 'network');
        assert.ok(h.lastError.at <= h.lastSyncAt, 'error predates the recovery');

        server.upsert(flag('a'));
        await until(() => c.getHealth().lastAppliedRevision === 4);
    } finally {
        c.disconnect();
        await server.stop();
    }
    assert.equal(c.getHealth().state, 'closed');
});

test('poll fallback, offline cache and auth failure', opts, async () => {
    const server = new FaultServer();
    const url = await server.start();
    server.upsert(flag('a'));
    const cache = JSON.stringify({ cached: flag('cached') });
    server.blockStreams = true; // streams answer 503, snapshots still work
    const c = new CheckgateClient({
        serverUrl: url, ...FAST, pollFallbackThreshold: 2,
        storage: { getItem: () => cache, setItem: () => {} },
    });
    try {
        const connecting = c.connect();
        await until(() => c.getHealth().offlineCache.hydrated);
        let h = c.getHealth();
        assert.equal(h.source, 'offline_cache');
        assert.equal(h.ready, false);
        await connecting;
        await until(() => c.getHealth().lastPollAt !== null);
        h = c.getHealth();
        assert.equal(h.polling, true);
        assert.equal(h.state, 'polling');
        assert.equal(h.source, 'server');
        assert.equal(h.lastError.kind, 'http');
        assert.ok(h.reconnectAttempts >= 2);

        server.setAcceptedKey(null);
        await until(() => c.getHealth().state === 'auth_failed');
        h = c.getHealth();
        assert.equal(h.lastError.kind, 'auth');
        assert.ok(!JSON.stringify(h).includes('sk_conformance'));
    } finally {
        c.disconnect();
        await server.stop();
    }
});

test('a silent stream is reported as a timeout', opts, async () => {
    const server = new FaultServer({ heartbeatMs: 50 });
    const url = await server.start();
    server.upsert(flag('a'));
    const c = new CheckgateClient({ serverUrl: url, ...FAST, pollFallbackThreshold: Infinity });
    try {
        await c.connect();
        server.stallAll();
        await until(() => c.getHealth().lastError && c.getHealth().lastError.kind === 'timeout');
        assert.equal(c.getHealth().flagCount, 1, 'held flags kept');
    } finally {
        c.disconnect();
        await server.stop();
    }
});
