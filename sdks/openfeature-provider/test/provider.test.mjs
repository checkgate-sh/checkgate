// CheckgateProvider against the real OpenFeature server SDK, the real
// @checkgate/node client with its native engine, and a real SSE stream from
// the conformance FaultServer (which speaks the server's SDK protocol).
// Needs the native addon (`cd ../nodejs && npm run build`); skipped otherwise.

import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { createRequire } from 'node:module';
import { FaultServer } from '../../conformance/fault-server.mjs';

const require = createRequire(import.meta.url);
const { OpenFeature, ProviderEvents, ProviderStatus, ErrorCode } = require('@openfeature/server-sdk');
const { CheckgateProvider, toAttributes } = require('../src/index.js');

let native = true;
try {
    require('@checkgate/node');
} catch {
    native = false;
}
const opts = { skip: native ? false : 'native addon not built' };

const vectors = JSON.parse(fs.readFileSync(new URL('../../conformance/vectors/evaluation.json', import.meta.url), 'utf8'));

// The client logs connection changes; keep the test output readable.
for (const level of ['log', 'warn']) console[level] = () => {};

const FAST = {
    reportImpressions: false, reconnectDelayMs: 20, maxReconnectDelayMs: 100,
    pollFallbackThreshold: Infinity, connectTimeoutMs: 500,
};

let domainSeq = 0;
async function setUp({ flags = [], providerOptions = {}, serverOptions = {}, start = true } = {}) {
    const server = new FaultServer(serverOptions);
    const url = await server.start();
    for (const f of flags) server.upsert(f);
    const provider = new CheckgateProvider({ serverUrl: url, sdkKey: 'sk_conformance', ...FAST, ...providerOptions });
    const domain = `test-${++domainSeq}`;
    if (start) await OpenFeature.setProviderAndWait(domain, provider);
    return { server, provider, domain, client: OpenFeature.getClient(domain) };
}

async function tearDown({ server }) {
    await OpenFeature.close();
    await server.stop();
}

function until(predicate, ms = 5000) {
    const started = Date.now();
    return new Promise((resolve, reject) => {
        const tick = () => {
            if (predicate()) return resolve();
            if (Date.now() - started > ms) return reject(new Error('condition not reached'));
            setTimeout(tick, 10);
        };
        tick();
    });
}

after(() => OpenFeature.close());

/** The OpenFeature resolution type for a vector flag. */
function kind(flag) {
    return { boolean: 'Boolean', string: 'String', integer: 'Number', json: 'Object' }[flag.flag_type || 'boolean'];
}
const DEFAULTS = { Boolean: false, String: 'dflt', Number: -1, Object: { dflt: true } };
const OF_REASON = {
    DISABLED: 'DISABLED', PREREQUISITE_FAILED: 'PREREQUISITE_FAILED', TARGETING_MATCH: 'TARGETING_MATCH',
    ROLLOUT_EXCLUDED: 'SPLIT', SPLIT: 'SPLIT', DEFAULT: 'DEFAULT', STATIC: 'STATIC',
};

test('every shared vector resolves through OpenFeature as the engine says', opts, async () => {
    // The flags arrive over the stream, as in production.
    const env = await setUp({ flags: vectors.flags });
    try {
        const byKey = new Map(vectors.flags.map((f) => [f.key, f]));
        let checked = 0;
        for (const c of vectors.cases) {
            const flag = byKey.get(c.flag);
            const type = flag ? kind(flag) : 'Boolean';
            const dflt = DEFAULTS[type];
            const ctx = { targetingKey: c.user, ...c.attributes };
            const d = await env.client[`get${type}Details`](c.flag, dflt, ctx);
            if (c.user === '') {
                // The provider requires a targeting key by default.
                assert.equal(d.errorCode, ErrorCode.TARGETING_KEY_MISSING, c.name);
                assert.deepEqual(d.value, dflt, c.name);
                continue;
            }
            if (c.expect === null) {
                assert.equal(d.errorCode, ErrorCode.FLAG_NOT_FOUND, c.name);
                assert.deepEqual(d.value, dflt, c.name);
                continue;
            }
            assert.equal(d.errorCode, undefined, `${c.name}: ${d.errorMessage}`);
            assert.equal(d.reason, OF_REASON[c.expect.reason], c.name);
            assert.equal(d.flagMetadata.checkgateReason, c.expect.reason, c.name);
            assert.equal(d.flagMetadata.ruleIndex, c.expect.rule_index, c.name);
            assert.equal(d.flagMetadata.variantIndex, c.expect.variant_index, c.name);
            assert.equal(d.flagMetadata.rolloutBucket, c.expect.rollout_bucket, c.name);
            const typed = { Boolean: 'boolean', String: 'string', Number: 'number', Object: 'object' }[type];
            const engineValue = c.expect.value;
            const usable = engineValue !== null && typeof engineValue === typed;
            if (usable) {
                assert.deepEqual(d.value, engineValue, c.name);
            } else {
                // Off (or no value configured) without a value of this type:
                // the caller's default, without an error.
                assert.ok(!c.expect.enabled || engineValue === null, `${c.name}: unexpected type mismatch`);
                assert.deepEqual(d.value, dflt, c.name);
            }
            // Parity with direct SDK use of the very same client.
            const direct = env.provider.client.getDetails(c.flag, c.user, c.attributes);
            assert.equal(direct.reason, c.expect.reason, c.name);
            checked++;
        }
        assert.ok(checked > 70, `checked ${checked}`);
    } finally {
        await tearDown(env);
    }
});

test('evaluations make no network request', opts, async () => {
    const env = await setUp({ flags: vectors.flags });
    try {
        const before = { ...env.server.counters };
        for (let i = 0; i < 2000; i++) {
            await env.client.getBooleanValue('rollout-30', false, { targetingKey: `user-${i}` });
            await env.client.getStringValue('multi-rule', 'x', { targetingKey: 'u', country: 'PH' });
        }
        assert.equal(env.server.counters.streamRequests, before.streamRequests);
        assert.equal(env.server.counters.snapshotRequests, before.snapshotRequests);
    } finally {
        await tearDown(env);
    }
});

test('a type mismatch returns the default with TYPE_MISMATCH', opts, async () => {
    const env = await setUp({ flags: vectors.flags });
    try {
        const d = await env.client.getBooleanDetails('multi-rule', true, { targetingKey: 'u', country: 'PH' });
        assert.equal(d.errorCode, ErrorCode.TYPE_MISMATCH);
        assert.equal(d.value, true);
        const n = await env.client.getNumberDetails('json-flag', 7, { targetingKey: 'u' });
        assert.equal(n.errorCode, ErrorCode.TYPE_MISMATCH);
        assert.equal(n.value, 7);
    } finally {
        await tearDown(env);
    }
});

test('context: numbers, booleans and dates become strings; objects are left out', () => {
    assert.deepEqual(
        toAttributes({ targetingKey: 'u', age: 30, beta: true, plan: 'pro', when: new Date(0), nested: { a: 1 }, list: [1], none: null, nan: NaN }),
        { age: '30', beta: 'true', plan: 'pro', when: '1970-01-01T00:00:00.000Z' },
    );
});

test('a numeric context attribute drives a numeric rule', opts, async () => {
    const env = await setUp({ flags: vectors.flags });
    try {
        assert.equal(await env.client.getBooleanValue('adults', false, { targetingKey: 'u', age: 30 }), true);
        assert.equal(await env.client.getBooleanValue('adults', true, { targetingKey: 'u', age: 17 }), false);
        // An object where a rule expects a string is dropped, not thrown on.
        const d = await env.client.getBooleanDetails('rules-plan', true, { targetingKey: 'u', plan: { tier: 'pro' } });
        assert.equal(d.errorCode, undefined);
        assert.equal(d.value, false);
    } finally {
        await tearDown(env);
    }
});

test('a missing targetingKey is an error unless allowed', opts, async () => {
    const env = await setUp({ flags: vectors.flags });
    try {
        const d = await env.client.getBooleanDetails('static-on', false, {});
        assert.equal(d.errorCode, ErrorCode.TARGETING_KEY_MISSING);
        assert.equal(d.value, false);
    } finally {
        await tearDown(env);
    }
    const lax = await setUp({ flags: vectors.flags, providerOptions: { requireTargetingKey: false } });
    try {
        assert.equal(await lax.client.getBooleanValue('static-on', false, {}), true);
    } finally {
        await tearDown(lax);
    }
});

test('an uninitialized provider answers PROVIDER_NOT_READY', opts, async () => {
    const provider = new CheckgateProvider({ serverUrl: 'http://127.0.0.1:9', ...FAST });
    await assert.rejects(provider.resolveBooleanEvaluation('f', false, { targetingKey: 'u' }), (e) => e.code === ErrorCode.PROVIDER_NOT_READY);
    await provider.onClose();
});

test('lifecycle: READY, STALE on a dropped stream, READY again, then offline evaluation', opts, async () => {
    const env = await setUp({ flags: vectors.flags });
    try {
        assert.equal(env.client.providerStatus, ProviderStatus.READY);
        const events = [];
        for (const e of [ProviderEvents.Ready, ProviderEvents.Stale, ProviderEvents.Error]) {
            env.client.addHandler(e, () => events.push(e));
        }
        // OpenFeature runs a Ready handler at once when the provider is already ready.
        assert.deepEqual(events, [ProviderEvents.Ready]);
        events.length = 0;
        env.server.mode = 'refuse';
        env.server.severAll();
        await until(() => env.client.providerStatus === ProviderStatus.STALE);
        // Held flags keep answering while disconnected.
        assert.equal(await env.client.getBooleanValue('static-on', false, { targetingKey: 'u' }), true);
        env.server.mode = 'pass';
        await until(() => env.client.providerStatus === ProviderStatus.READY);
        assert.deepEqual(events, [ProviderEvents.Stale, ProviderEvents.Ready]);
    } finally {
        await tearDown(env);
    }
});

test('a flag change emits one ConfigurationChanged with the flag key', opts, async () => {
    const env = await setUp({ flags: vectors.flags });
    try {
        const changes = [];
        env.client.addHandler(ProviderEvents.ConfigurationChanged, (d) => changes.push(d.flagsChanged));
        env.server.upsert({ key: 'static-on', is_enabled: false, rollout_percentage: null, rules: [] });
        await until(() => changes.length > 0);
        assert.deepEqual(changes, [['static-on']]);
        assert.equal(await env.client.getBooleanValue('static-on', true, { targetingKey: 'u' }), false);
    } finally {
        await tearDown(env);
    }
});

test('a rejected SDK key makes the provider FATAL', opts, async () => {
    const env = await setUp({ flags: vectors.flags, start: false });
    try {
        env.server.setAcceptedKey(null);
        await assert.rejects(OpenFeature.setProviderAndWait(env.domain, env.provider));
        assert.equal(env.client.providerStatus, ProviderStatus.FATAL);
        const d = await env.client.getBooleanDetails('static-on', false, { targetingKey: 'u' });
        assert.equal(d.errorCode, ErrorCode.PROVIDER_FATAL);
    } finally {
        await tearDown(env);
    }
});

test('initTimeoutMs: ERROR while unreachable, READY once the server answers', opts, async () => {
    const env = await setUp({ flags: vectors.flags, start: false, providerOptions: { initTimeoutMs: 300 } });
    try {
        env.server.mode = 'refuse';
        await assert.rejects(OpenFeature.setProviderAndWait(env.domain, env.provider));
        assert.equal(env.client.providerStatus, ProviderStatus.ERROR);
        env.server.mode = 'pass';
        await until(() => env.client.providerStatus === ProviderStatus.READY);
        assert.equal(await env.client.getBooleanValue('static-on', false, { targetingKey: 'u' }), true);
    } finally {
        await tearDown(env);
    }
});

test('initTimeoutMs with an offline cache: STALE on cached flags, READY once connected', opts, async () => {
    const cache = JSON.stringify({ cached: { key: 'cached', is_enabled: true, rollout_percentage: null, rules: [] } });
    const env = await setUp({
        flags: [{ key: 'live', is_enabled: true, rollout_percentage: null, rules: [] }],
        start: false,
        providerOptions: { initTimeoutMs: 300, storage: { getItem: () => cache, setItem: () => {} } },
    });
    try {
        env.server.mode = 'refuse';
        await OpenFeature.setProviderAndWait(env.domain, env.provider);
        await until(() => env.client.providerStatus === ProviderStatus.STALE);
        assert.equal(await env.client.getBooleanValue('cached', false, { targetingKey: 'u' }), true);
        env.server.mode = 'pass';
        await until(() => env.client.providerStatus === ProviderStatus.READY);
        assert.equal(await env.client.getBooleanValue('live', false, { targetingKey: 'u' }), true);
    } finally {
        await tearDown(env);
    }
});

test('close disconnects an owned client but never a shared one', opts, async () => {
    const owned = await setUp({ flags: vectors.flags });
    await until(() => owned.server.streams.size === 1);
    await OpenFeature.close();
    await until(() => owned.server.streams.size === 0);
    await owned.server.stop();

    const server = new FaultServer();
    const url = await server.start();
    server.upsert({ key: 'f', is_enabled: true, rollout_percentage: null });
    const { CheckgateClient } = require('@checkgate/node');
    const shared = new CheckgateClient({ serverUrl: url, sdkKey: 'sk_conformance', ...FAST });
    await shared.connect();
    await OpenFeature.setProviderAndWait('shared', new CheckgateProvider({ client: shared }));
    assert.equal(await OpenFeature.getClient('shared').getBooleanValue('f', false, { targetingKey: 'u' }), true);
    await OpenFeature.close();
    assert.equal(shared.isEnabled('f', 'u'), true, 'shared client still serving');
    assert.equal(shared.getStatus().state, 'streaming');
    shared.disconnect();
    await server.stop();
});

test('track() forwards to the client as a goal event', opts, () => {
    const calls = [];
    const fake = { track: (...a) => calls.push(a), onChange: () => () => {}, onStatusChange: () => () => {}, isReady: () => true };
    const provider = new CheckgateProvider({ client: fake });
    provider.track('checkout', { targetingKey: 'u1' }, { value: 9.5, sku: 'x' });
    provider.track('ignored', {}, {});
    assert.deepEqual(calls, [['checkout', 'u1', { value: 9.5, context: { sku: 'x' } }]]);
});
