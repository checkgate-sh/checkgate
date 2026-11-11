// Local evaluation through the @checkgate/node public API (isEnabled /
// getValue), which includes the N-API boundary: argument conversion of the
// flag key, user key and attribute object, and for getValue the JSON result.
//
//   node eval.mjs --out evaluation-node.json
//
// Setup is excluded from timing: the client bootstraps over its real SSE code
// path from a local stub stream (the same `connected` / `update` / `ready`
// events the server sends), then disconnects. The timed loops run with fetch
// instrumented and the stub closed; any network attempt fails the run.
//
// Method: per case, a warm-up, then `samples` batches of `batch` calls each,
// timed with process.hrtime.bigint(). Each sample is a batch mean, so the
// percentiles describe the spread of batch means, not single calls (a single
// call is shorter than the timer's resolution and overhead).

import http from 'node:http';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { evaluationFlags, USER_KEY, USER_ATTRIBUTES } from './fixtures.mjs';
import { parseArgs, pct, round3 } from './util.mjs';

const args = parseArgs({ samples: '100', batch: '20000', warmup: '200000', out: '' });
const SAMPLES = Number(args.samples);
const BATCH = Number(args.batch);
const WARMUP = Number(args.warmup);

const require = createRequire(import.meta.url);
const here = path.dirname(fileURLToPath(import.meta.url));
const sdkPath = process.env.CHECKGATE_SDK || path.resolve(here, '../../../sdks/nodejs/index.js');
const { CheckgateClient } = require(sdkPath);

// ── Stub stream: bootstrap only ─────────────────────────────────────────────

let stubRequests = 0;
const flags = evaluationFlags();
const stub = http.createServer((req, res) => {
  stubRequests++;
  if (req.url !== '/stream') {
    res.writeHead(404).end();
    return;
  }
  res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache' });
  res.write(`event: connected\ndata: ${JSON.stringify({ environment_id: 'bench-env' })}\n\n`);
  flags.forEach((flag, i) => {
    res.write(`event: update\ndata: ${JSON.stringify({ type: 'UPSERT', env_id: 'bench-env', revision: i + 1, flag })}\n\n`);
  });
  res.write(`event: ready\ndata: ${flags.length}\n\n`);
});
await new Promise((r) => stub.listen(0, '127.0.0.1', r));
const serverUrl = `http://127.0.0.1:${stub.address().port}`;

const quiet = console.log;
console.log = () => {}; // the SDK logs each (re)connect
async function bootstrapped() {
  const c = new CheckgateClient({ serverUrl, sdkKey: 'bench', reportImpressions: false });
  await c.connect();
  c.disconnect(); // flags stay loaded; evaluation is local from here on
  return c;
}
const client = await bootstrapped();
const second = await bootstrapped(); // an independent instance, for determinism checks
console.log = quiet;
stub.closeAllConnections();
await new Promise((r) => stub.close(r));
const requestsAfterSetup = stubRequests;

let fetchCalls = 0;
const realFetch = globalThis.fetch;
globalThis.fetch = (...a) => {
  fetchCalls++;
  return realFetch(...a);
};

// ── Cases ───────────────────────────────────────────────────────────────────

const attrs = { ...USER_ATTRIBUTES };
const cases = [
  ['boolean', 'isEnabled', () => client.isEnabled('flag-500', USER_KEY, attrs), true],
  ['targeting_1_rule', 'isEnabled', () => client.isEnabled('t1', USER_KEY, attrs), true],
  ['targeting_10_rules', 'isEnabled', () => client.isEnabled('t10', USER_KEY, attrs), true],
  ['percentage_rollout', 'isEnabled', () => client.isEnabled('rollout', USER_KEY, attrs), null],
  ['weighted_multivariate', 'getValue', () => client.getValue('mv', USER_KEY, attrs), null],
  ['prerequisite_chain_5', 'isEnabled', () => client.isEnabled('chain-0', USER_KEY, attrs), true],
];

// Correctness: expected results, and identical results across calls and
// across two independently bootstrapped clients (deterministic bucketing).
const checks = {};
for (const [name, , fn, expected] of cases) {
  const first = fn();
  if (expected !== null && first !== expected) throw new Error(`${name}: expected ${expected}, got ${first}`);
  if (name === 'weighted_multivariate' && !['a', 'b', 'c'].includes(first)) throw new Error(`${name}: got ${first}`);
  for (let i = 0; i < 1000; i++) if (fn() !== first) throw new Error(`${name}: result changed between calls`);
  checks[name] = first;
}
const rolloutOther = second.isEnabled('rollout', USER_KEY, attrs);
const mvOther = second.getValue('mv', USER_KEY, attrs);
if (rolloutOther !== checks.percentage_rollout || mvOther !== checks.weighted_multivariate) {
  throw new Error('bucketing differs between two client instances');
}
// Bucketing spreads users: across 10 000 user keys the 50 % rollout lands near 50 %.
let on = 0;
for (let i = 0; i < 10_000; i++) if (client.isEnabled('rollout', `user-${i}`, attrs)) on++;
if (on < 4_700 || on > 5_300) throw new Error(`rollout split ${on}/10000 is not ~50 %`);
const absent = client.isEnabled('absent-flag', USER_KEY, attrs);
if (absent !== false) throw new Error('absent flag evaluated true');

// ── Timing ──────────────────────────────────────────────────────────────────

let sink = 0; // consumed below so no call can be optimized away
function run(fn) {
  for (let i = 0; i < WARMUP; i++) {
    const r = fn();
    sink += r === true ? 1 : typeof r === 'string' ? r.length : 0;
  }
  const perCall = [];
  for (let s = 0; s < SAMPLES; s++) {
    const t = process.hrtime.bigint();
    for (let i = 0; i < BATCH; i++) {
      const r = fn();
      sink += r === true ? 1 : typeof r === 'string' ? r.length : 0;
    }
    perCall.push(Number(process.hrtime.bigint() - t) / BATCH);
  }
  return perCall;
}

function describe(perCall) {
  const s = Float64Array.from(perCall).sort();
  const n = s.length;
  const mean = s.reduce((a, b) => a + b, 0) / n;
  const sd = Math.sqrt(s.reduce((a, b) => a + (b - mean) ** 2, 0) / (n - 1));
  const half = (1.96 * sd) / Math.sqrt(n);
  const med = pct(s, 0.5);
  return {
    samples: n, calls_per_sample: BATCH,
    median_ns: round3(med), mean_ns: round3(mean),
    mean_ci95_ns: [round3(mean - half), round3(mean + half)],
    p95_ns: round3(pct(s, 0.95)), p99_ns: round3(pct(s, 0.99)),
    min_ns: round3(s[0]), max_ns: round3(s[n - 1]),
    stddev_ns: round3(sd),
    ops_per_s: Math.round(1e9 / med),
  };
}

const results = {};
for (const [name, api, fn] of cases) {
  results[name] = { api, ...describe(run(fn)) };
  console.error(`${name.padEnd(24)} ${results[name].median_ns} ns  (${results[name].ops_per_s.toLocaleString()} ops/s)`);
}

if (fetchCalls !== 0 || stubRequests !== requestsAfterSetup) {
  throw new Error(`network activity during timing: ${fetchCalls} fetch calls`);
}

const out = {
  generated_by: 'bench/public/sdk/eval.mjs',
  sdk: '@checkgate/node public API (index.js → N-API → checkgate-core), reportImpressions=false',
  store_flags: flags.length,
  user: { key: USER_KEY, attributes: Object.keys(USER_ATTRIBUTES).length },
  method: 'warm-up, then batches of calls timed with process.hrtime.bigint(); each sample is a batch mean',
  warmup_calls: WARMUP,
  node: process.version,
  v8: process.versions.v8,
  platform: `${os.platform()} ${os.arch()}`,
  cpu: os.cpus()[0]?.model,
  checks: { ...checks, rollout_split_of_10000: on, absent_flag: absent, network_calls_during_timing: fetchCalls },
  cases: results,
  sink,
};
const text = JSON.stringify(out, null, 2);
if (args.out) writeFileSync(args.out, text);
else console.log(text);
