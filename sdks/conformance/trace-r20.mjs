// Instrumented run of one seed against the Node SDK: prints, in order, every
// stream request, poll response, `connected`, `ready` swap and applied delta
// with the revisions involved. Used to document R20
// (docs/releases/v0.1.31/RC3-R20-ROOT-CAUSE.md); not part of any suite.
//
//   node sdks/conformance/trace-r20.mjs 173 [path/to/sdks/nodejs/index.js]
//   node sdks/conformance/trace-r20.mjs 'name:seed 184' …   (a named scenario)

import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { NAMED, generateScenario, runScenario } from './scenario.mjs';

const which = process.argv[2] || '173';
const named = which.startsWith('name:')
    ? Object.entries(NAMED).find(([n]) => n.includes(which.slice(5)))
    : null;
const seed = named ? 0 : Number(which);
const entry = process.argv[3] || fileURLToPath(new URL('../nodejs/index.js', import.meta.url));
const require = createRequire(import.meta.url);
const { CheckgateClient } = require(entry);

const t0 = Date.now();
const events = [];
const log = (what) => events.push(`${String(Date.now() - t0).padStart(5)} ms  ${what}`);
const revOf = (flag) => (flag && flag.description ? flag.description.replace('rev:', '@') : '@?');
const summary = (flags) => [...flags].map((f) => `${f.key}${revOf(f)}`).sort().join(' ');

const realFetch = globalThis.fetch;
globalThis.fetch = async (url, init) => {
    const u = String(url);
    if (u.includes('/stream')) log(`stream request sent`);
    const res = await realFetch(url, init);
    if (u.includes('/flags/snapshot') && res.ok) {
        const body = await res.clone().json();
        const flags = Array.isArray(body) ? body : body.flags;
        log(`poll response: ${summary(flags)}`);
    }
    return res;
};

const methods = ['_applyDelta', '_applySnapshot', '_applyFullSet'];
const proto = CheckgateClient.prototype;
for (const name of methods) {
    const original = proto[name];
    if (typeof original !== 'function') continue;
    proto[name] = function (...args) {
        if (name === '_applyDelta') {
            const e = args[0] || {};
            const key = e.type === 'UPSERT' ? e.flag && e.flag.key : e.key;
            log(`${this._staged ? 'stage' : 'apply'} ${e.type} ${key}@${e.revision}`);
        } else {
            const before = summary(this._flagCache.values());
            const out = original.apply(this, args);
            log(`${name}: ${before}  ->  ${summary(this._flagCache.values())}`);
            return out;
        }
        return original.apply(this, args);
    };
}

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
    status: (c) => c.getStatus(),
    close: (c) => c.disconnect(),
};

let outcome = 'pass';
try {
    const scenario = named ? { seed: 0, ...named[1] } : generateScenario(seed);
    await runScenario(adapter, scenario, { label: `trace-${named ? named[0] : seed}` });
} catch (err) {
    outcome = err.message;
}
process.stdout.write(events.join('\n') + `\n\n${named ? named[0] : `seed ${seed}`}: ${outcome}\n`);
