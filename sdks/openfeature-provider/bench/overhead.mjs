// Provider overhead: the same evaluation through OpenFeature vs. the Checkgate
// client directly, same process, same flags. Not part of `npm test`.
//   node bench/overhead.mjs [iterations]
import { createRequire } from 'node:module';
import { FaultServer } from '../../conformance/fault-server.mjs';
const require = createRequire(import.meta.url);
const { OpenFeature } = require('@openfeature/server-sdk');
const { CheckgateProvider } = require('../src/index.js');
for (const level of ['log', 'warn']) console[level] = () => {};

const N = Number(process.argv[2] || 200_000);
const server = new FaultServer();
const url = await server.start();
server.upsert({ key: 'plain', is_enabled: true, rollout_percentage: null, rules: [] });
server.upsert({ key: 'targeted', is_enabled: true, rollout_percentage: 50, rules: [{ attribute: 'plan', operator: 'equals', values: ['enterprise'] }] });
const provider = new CheckgateProvider({ serverUrl: url, sdkKey: 'sk_conformance', reportImpressions: false });
await OpenFeature.setProviderAndWait(provider);
const of = OpenFeature.getClient();
const direct = provider.client;

async function time(label, fn) {
    for (let i = 0; i < 20_000; i++) await fn(i); // warm-up
    const runs = [];
    for (let r = 0; r < 5; r++) {
        const t = process.hrtime.bigint();
        for (let i = 0; i < N; i++) await fn(i);
        runs.push(Number(process.hrtime.bigint() - t) / N);
    }
    runs.sort((a, b) => a - b);
    console.error(`${label.padEnd(44)} median ${runs[2].toFixed(0)} ns/op  (min ${runs[0].toFixed(0)}, max ${runs[4].toFixed(0)})`);
    return runs[2];
}
const ctx = { targetingKey: 'user-1', plan: 'pro', country: 'PH' };
const attrs = { plan: 'pro', country: 'PH' };
const out = {};
out.direct_isEnabled_plain = await time('direct isEnabled (plain)', () => direct.isEnabled('plain', 'user-1', attrs));
out.direct_getDetails_plain = await time('direct getDetails (plain)', () => direct.getDetails('plain', 'user-1', attrs));
out.of_getBooleanValue_plain = await time('OpenFeature getBooleanValue (plain)', () => of.getBooleanValue('plain', false, ctx));
out.direct_isEnabled_targeted = await time('direct isEnabled (rule + 50% rollout)', (i) => direct.isEnabled('targeted', `user-${i & 1023}`, attrs));
out.direct_getDetails_targeted = await time('direct getDetails (rule + 50% rollout)', (i) => direct.getDetails('targeted', `user-${i & 1023}`, attrs));
out.of_getBooleanValue_targeted = await time('OpenFeature getBooleanValue (rule + rollout)', (i) => of.getBooleanValue('targeted', false, { ...ctx, targetingKey: `user-${i & 1023}` }));
console.log(JSON.stringify({ node: process.version, iterations: N, ns_per_op: out }));
await OpenFeature.close();
await server.stop();
