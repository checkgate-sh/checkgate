// Fault-injection suite for the Node SDK: the real client and the real
// `eventsource` transport against ../conformance/fault-server.mjs. Uses the
// native engine when it is built (required with CHECKGATE_NATIVE=1, skipped
// with CHECKGATE_NATIVE=0), else a
// deterministic stand-in. See sdks/conformance/README.md.
//
//   node --test test/faults.test.mjs                       # CI seeds
//   CONFORMANCE_SEEDS=1-500 node --test test/faults.test.mjs  # extended

import { test } from 'node:test';
import assert from 'node:assert';
import { createRequire } from 'node:module';
import fs from 'node:fs';
import { CI_SEEDS, NAMED, generateScenario, runScenario, seedsFromEnv } from '../../conformance/scenario.mjs';
import { makeFakeCore } from '../../conformance/fake-core.mjs';

const require = createRequire(import.meta.url);
const Module = require('node:module');
const here = new URL('..', import.meta.url).pathname;
const native = process.env.CHECKGATE_NATIVE !== '0' && fs.readdirSync(here).some((f) => f.endsWith('.node'));
if (process.env.CHECKGATE_NATIVE === '1') assert.ok(native, 'CHECKGATE_NATIVE=1 but no native addon is built');

if (!native) {
    const origLoad = Module._load;
    Module._load = function (request, ...rest) {
        if (request === './native-binding.js') {
            return {
                CheckgateCore: function () {
                    const core = makeFakeCore();
                    return {
                        upsertFlagV2: (json) => core.upsert(JSON.parse(json)),
                        deleteFlag: (key) => core.delete(key),
                        isEnabled: (key, user, attrs) => core.isEnabled(key, user, attrs),
                        getVariant: () => 'null',
                    };
                },
            };
        }
        return origLoad.call(this, request, ...rest);
    };
}
// CONFORMANCE_NODE_ENTRY runs the suite against another build of the client
// (e.g. an older release, to show which of these failures it had).
const { CheckgateClient } = require(process.env.CONFORMANCE_NODE_ENTRY || '../index.js');

// The SDK logs every reconnect; hundreds of injected faults would bury the report.
for (const level of ['log', 'warn', 'error']) console[level] = () => {};

const adapter = {
    create: (url, sdkKey, t) => new CheckgateClient({
        serverUrl: url, sdkKey, reportImpressions: false,
        reconnectDelayMs: t.reconnectDelayMs, maxReconnectDelayMs: t.maxReconnectDelayMs,
        pollFallbackThreshold: t.pollFallbackThreshold, pollIntervalMs: t.pollIntervalMs,
        connectTimeoutMs: t.connectTimeoutMs,
    }),
    connect: (c) => c.connect(),
    flags: (c) => new Map(c._flagCache),
    evaluate: (c, key, user) => c.isEnabled(key, user, { plan: 'pro' }),
    status: (c) => (c.getStatus ? c.getStatus() : { state: 'unknown (no getStatus)' }),
    close: (c) => c.disconnect(),
};

const engine = native ? 'native engine' : 'stand-in engine';

for (const [name, scenario] of Object.entries(NAMED)) {
    test(`[node, ${engine}] ${name}`, async () => {
        await runScenario(adapter, { seed: 0, ...scenario }, { label: `node-${name}` });
    });
}

for (const seed of seedsFromEnv(CI_SEEDS)) {
    test(`[node, ${engine}] random seed ${seed}`, async () => {
        await runScenario(adapter, generateScenario(seed), { label: `node-seed-${seed}` });
    });
}
