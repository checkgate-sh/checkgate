// Flag-update propagation through real SDKs.
//
//   node propagation.mjs --session /work/session.json --clients 1,100,500,1000 \
//       --updates 200 --out /results/propagation.json
//
// For each profile, worker processes (propagation-worker.mjs) open N
// @checkgate/node clients and wait until every one has bootstrapped and
// evaluates the current value. The coordinator then sends one PATCH at a time,
// setting the `hot` string flag's default_value to a value never used before,
// and waits until every client has observed it through getValue() (or a
// timeout) before the next update. No two updates overlap. Each profile
// starts with --warmup-updates unrecorded updates (the first writes after
// connecting take a cold path).
//
// Per (update, client) it records, on the shared monotonic clock:
//   visible_from_request   = SDK getValue() returns the new value − PATCH sent
//                            (primary metric)
//   visible_after_response = the same, minus the PATCH response time; 0 when
//                            the value was visible before the response arrived
//                            (the server publishes before it responds)

import { fork } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { HOT_FLAG } from './fixtures.mjs';
import { parseArgs, list, summarize, histogram, procUsage, sleep, mib, round3 } from './util.mjs';

const args = parseArgs({
  session: '/work/session.json',
  clients: '1,100,500,1000',
  updates: '200',
  'warmup-updates': '5',
  'clients-per-worker': '125',
  'max-workers': '8',
  'gap-ms': '100',
  'deadline-ms': '1000',
  'timeout-ms': '10000',
  'hold-s': '5',
  'server-pid': '1',
  'run-tag': `r${process.pid}`,
  out: '',
});
const session = JSON.parse(readFileSync(args.session, 'utf8'));
const url = session.url;
const updates = Number(args.updates);
const warmupUpdates = Number(args['warmup-updates']);
const gapMs = Number(args['gap-ms']);
const deadlineMs = Number(args['deadline-ms']);
const timeoutMs = Number(args['timeout-ms']);
const serverPid = args['server-pid'];
const here = path.dirname(fileURLToPath(import.meta.url));
const now = () => process.hrtime.bigint();
const msBetween = (a, b) => Number(b - a) / 1e6;

async function patchHot(value) {
  const t0 = now();
  const res = await fetch(`${url}/api/environments/${session.environment_id}/flags/${HOT_FLAG}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json', 'X-Checkgate-Request': 'true', Cookie: session.cookie },
    body: JSON.stringify({ default_value: value }),
  });
  await res.text();
  const t1 = now();
  if (res.status !== 200) throw new Error(`PATCH ${HOT_FLAG}: HTTP ${res.status}`);
  return { t0, t1 };
}

function spawnWorker() {
  const w = fork(path.join(here, 'propagation-worker.mjs'), [], { stdio: ['ignore', 'ignore', 'inherit', 'ipc'] });
  w.inbox = [];
  w.waiters = [];
  return w;
}

function nextMessage(w, type, ms = 180_000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`worker: no ${type} within ${ms} ms`)), ms);
    const check = (m) => {
      if (m.t !== type) return false;
      clearTimeout(timer);
      resolve(m);
      return true;
    };
    w.waiters.push(check);
  });
}

async function profile(n, currentValue) {
  // Many SDK instances in one Node process handle each update one after
  // another; cap that so the harness, not the server, never sets the tail.
  const workerCount = Math.min(Number(args['max-workers']), Math.ceil(n / Number(args['clients-per-worker'])));
  const observations = new Map(); // value -> Map(clientId -> ts)
  let onObservation = () => {};
  const workers = [];
  for (let i = 0; i < workerCount; i++) {
    const w = spawnWorker();
    w.on('message', (m) => {
      if (m.t === 'obs') {
        for (const o of m.batch) {
          let byClient = observations.get(o.v);
          if (!byClient) observations.set(o.v, (byClient = new Map()));
          if (!byClient.has(o.c)) byClient.set(o.c, BigInt(o.ts));
          onObservation(o.v);
        }
        return;
      }
      w.waiters = w.waiters.filter((check) => !check(m));
    });
    workers.push(w);
  }

  // Connect: N clients split evenly; at most 50 handshakes in flight per worker.
  const cpu0 = procUsage(serverPid);
  const rampStart = now();
  let base = 0;
  const readies = workers.map((w, i) => {
    const count = Math.floor(n / workerCount) + (i < n % workerCount ? 1 : 0);
    const ready = nextMessage(w, 'ready');
    w.send({ cmd: 'start', url, sdkKey: session.sdk_key, count, base, connectConcurrency: 50 });
    base += count;
    return ready;
  });
  const ready = await Promise.all(readies);
  const rampS = msBetween(rampStart, now()) / 1000;
  const readyMs = ready.flatMap((r) => r.readyMs);
  const failures = ready.flatMap((r) => r.failures);
  const values = ready.flatMap((r) => r.values);
  const wrongStart = values.filter((v) => v !== currentValue).length;
  const connected = readyMs.length;
  const initialConnectedLogs = ready.reduce((s, r) => s + r.connectedLogs, 0);

  // Held idle: server memory and CPU with N open streams.
  await sleep(Number(args['hold-s']) * 1000);
  const held = procUsage(serverPid);

  // One update: PATCH, then wait until every client sees it (or the timeout),
  // then a jittered gap so updates never overlap.
  async function update(value) {
    const all = new Promise((resolve) => {
      const timer = setTimeout(resolve, timeoutMs);
      onObservation = (v) => {
        if (v === value && (observations.get(v)?.size ?? 0) >= connected) {
          clearTimeout(timer);
          resolve();
        }
      };
    });
    const { t0, t1 } = await patchHot(value);
    await all;
    onObservation = () => {};
    await sleep(gapMs + Math.floor(Math.random() * (gapMs / 2)));
    return { value, t0, t1 };
  }

  // Warm-up: the first writes after connecting take a cold path (connection
  // pools, statement caches) and are not recorded.
  let value = currentValue;
  for (let k = 1; k <= warmupUpdates; k++) {
    value = `${args['run-tag']}-n${n}-w${k}`;
    await update(value);
  }

  // Propagation.
  const sends = [];
  const cpuProp0 = procUsage(serverPid);
  const propStart = now();
  for (let k = 1; k <= updates; k++) {
    value = `${args['run-tag']}-n${n}-u${k}`;
    sends.push(await update(value));
  }
  await sleep(1000); // late arrivals
  const propS = msBetween(propStart, now()) / 1000;
  const cpuProp1 = procUsage(serverPid);

  // Collect per-worker stats, then stop.
  const stats = await Promise.all(workers.map((w) => {
    const p = nextMessage(w, 'stats');
    w.send({ cmd: 'stats' });
    return p;
  }));
  await Promise.all(workers.map((w) => {
    const p = nextMessage(w, 'stopped', 30_000).catch(() => null);
    w.send({ cmd: 'stop' });
    return p;
  }));

  const fromRequest = [];
  const afterResponse = [];
  const perUpdate = [];
  let missed = 0;
  let beforeResponse = 0;
  let withinDeadline = 0;
  for (const s of sends) {
    const byClient = observations.get(s.value) || new Map();
    missed += connected - byClient.size;
    const lat = [];
    for (const ts of byClient.values()) {
      const v = msBetween(s.t0, ts);
      lat.push(v);
      fromRequest.push(v);
      if (v <= deadlineMs) withinDeadline++;
      if (ts < s.t1) beforeResponse++;
      afterResponse.push(ts < s.t1 ? 0 : msBetween(s.t1, ts));
    }
    const sm = summarize(lat);
    perUpdate.push({
      response_ms: round3(msBetween(s.t0, s.t1)), observed: byClient.size,
      p50_ms: sm.p50_ms ?? null, p99_ms: sm.p99_ms ?? null, max_ms: sm.max_ms ?? null,
    });
  }
  const expected = sends.length * connected;
  const workerCpu = stats.reduce((s, w) => s + w.cpu_seconds, 0);
  const reconnects = stats.reduce((s, w) => s + w.connected_logs, 0) - initialConnectedLogs;
  const result = {
    clients: n,
    worker_processes: workerCount,
    connect: {
      seconds: round3(rampS),
      connected,
      failures,
      time_to_ready: summarize(readyMs),
      clients_with_wrong_initial_value: wrongStart,
    },
    held_idle: {
      server_rss_mib: mib(held?.rss),
      server_cpu_cores_during_connect_and_hold:
        cpu0 && held ? round3((held.cpu - cpu0.cpu) / (rampS + Number(args['hold-s']))) : null,
    },
    propagation: {
      warmup_updates_unrecorded: warmupUpdates,
      updates: sends.length,
      expected_observations: expected,
      observed: expected - missed,
      missed,
      observed_before_patch_response: beforeResponse,
      deadline_ms: deadlineMs,
      within_deadline_pct: expected ? round3((100 * withinDeadline) / expected) : null,
      visible_from_request: summarize(fromRequest),
      visible_after_response: summarize(afterResponse),
      patch_response: summarize(sends.map((s) => msBetween(s.t0, s.t1))),
      visible_from_request_histogram_0_1ms: histogram(fromRequest),
      per_update: perUpdate,
    },
    resources: {
      server_cpu_cores_during_propagation:
        cpuProp0 && cpuProp1 ? round3((cpuProp1.cpu - cpuProp0.cpu) / propS) : null,
      server_rss_mib_after_propagation: mib(cpuProp1?.rss),
      sdk_workers_cpu_cores: round3(workerCpu / (propS + Number(args['hold-s']))),
      sdk_workers_rss_mib: mib(stats.reduce((s, w) => s + w.rss_bytes, 0)),
    },
    stream_health: {
      reconnects,
      disconnect_warnings: stats.reduce((s, w) => s + w.disconnected_logs, 0),
      clients_polling_at_end: stats.reduce((s, w) => s + w.polling, 0),
      clients_ready_at_end: stats.reduce((s, w) => s + w.ready, 0),
    },
  };
  return { result, value };
}

const profiles = [];
let current = JSON.parse(readFileSync(args.session, 'utf8')).hot_value || 'u0';
for (const n of list(args.clients)) {
  console.error(`${n} clients…`);
  const { result, value } = await profile(n, current);
  current = value;
  const p = result.propagation;
  console.error(`  p50 ${p.visible_from_request.p50_ms} p99 ${p.visible_from_request.p99_ms} ms, missed ${p.missed}, reconnects ${result.stream_health.reconnects}`);
  profiles.push(result);
  await sleep(3000);
}
const hwm = procUsage(serverPid);
const out = {
  generated_by: 'bench/public/sdk/propagation.mjs',
  sdk: '@checkgate/node (index.js + N-API core), default options except reportImpressions=false',
  primary_metric: 'visible_from_request: PATCH request start → new value returned by the SDK getValue()',
  gap_ms: gapMs,
  server_rss_mib_high_water_mark: mib(hwm?.hwm),
  profiles,
};
const text = JSON.stringify(out, null, 2);
if (args.out) writeFileSync(args.out, text);
else console.log(text);
process.exit(0);
