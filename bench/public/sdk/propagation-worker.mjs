// One worker process of the propagation benchmark: holds `count` real
// @checkgate/node clients, each with its own SSE stream, and reports every
// new value of the hot flag the moment the SDK's public evaluation API
// returns it.
//
// Timestamps are process.hrtime.bigint() — CLOCK_MONOTONIC, which every
// process on one kernel shares — so the coordinator can subtract them from
// its own send times without any clock synchronization.

import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { HOT_FLAG } from './fixtures.mjs';

const require = createRequire(import.meta.url);
const here = path.dirname(fileURLToPath(import.meta.url));
const sdkPath = process.env.CHECKGATE_SDK || path.resolve(here, '../../../sdks/nodejs/index.js');
const { CheckgateClient } = require(sdkPath);

const EVAL_USER = 'bench-user';

// The SDK reports connection state through console messages only; count them.
const counters = { connected: 0, disconnected: 0 };
const origLog = console.log;
const origWarn = console.warn;
console.log = (...a) => {
  if (String(a[0]).includes('Stream connected')) counters.connected++;
  else origLog(...a);
};
console.warn = (...a) => {
  if (String(a[0]).includes('Stream disconnected')) counters.disconnected++;
  else origWarn(...a);
};

const clients = [];
let cpuStart = null;

// Observations are timestamped when they happen and sent in one IPC message
// per event-loop turn, so reporting costs little and never shifts a timestamp.
let pending = [];
function report(obs) {
  if (pending.length === 0) setImmediate(() => {
    process.send({ t: 'obs', batch: pending });
    pending = [];
  });
  pending.push(obs);
}

async function start({ url, sdkKey, count, base, connectConcurrency }) {
  const readyMs = [];
  const failures = [];
  let next = 0;
  async function connectOne() {
    while (next < count) {
      const i = next++;
      const client = new CheckgateClient({ serverUrl: url, sdkKey, reportImpressions: false });
      clients[i] = client;
      const t = process.hrtime.bigint();
      try {
        await Promise.race([
          client.connect(),
          new Promise((_, rej) => setTimeout(() => rej(new Error('connect timeout')), 120_000).unref()),
        ]);
        readyMs.push(Number(process.hrtime.bigint() - t) / 1e6);
      } catch (e) {
        failures.push(String(e && e.message));
        continue;
      }
      let last = client.getValue(HOT_FLAG, EVAL_USER);
      const id = base + i;
      client.onChange((key) => {
        if (key !== HOT_FLAG) return;
        // The value as an application sees it, through the public API.
        const v = client.getValue(HOT_FLAG, EVAL_USER);
        const ts = process.hrtime.bigint();
        if (v === last) return;
        last = v;
        report({ c: id, v, ts: ts.toString() });
      });
    }
  }
  await Promise.all(Array.from({ length: Math.min(connectConcurrency, count) }, connectOne));
  const values = clients.filter(Boolean).map((c) => c.getValue(HOT_FLAG, EVAL_USER));
  cpuStart = process.cpuUsage();
  process.send({ t: 'ready', readyMs, failures, values, connectedLogs: counters.connected });
}

function stats() {
  const cpu = process.cpuUsage(cpuStart);
  return {
    t: 'stats',
    connected_logs: counters.connected,
    disconnected_logs: counters.disconnected,
    polling: clients.filter((c) => c && c.isPolling()).length,
    ready: clients.filter((c) => c && c.isReady()).length,
    cpu_seconds: (cpu.user + cpu.system) / 1e6,
    rss_bytes: process.memoryUsage().rss,
  };
}

process.on('message', async (m) => {
  if (m.cmd === 'start') await start(m);
  else if (m.cmd === 'stats') process.send(stats());
  else if (m.cmd === 'stop') {
    for (const c of clients) if (c) c.disconnect();
    process.send({ t: 'stopped' }, () => process.exit(0));
  }
});
