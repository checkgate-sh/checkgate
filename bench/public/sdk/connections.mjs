// Connected-client cost workloads: what N streaming clients cost the server,
// and what the same N clients cost when they poll instead.
//
//   node connections.mjs --session /work/session.json --mode sse --clients 1000 \
//       --out /results/cost-sse-1000-run1.json
//   node connections.mjs --session /work/session.json --mode poll --clients 1000 \
//       --interval-s 30 --out /results/cost-poll-1000-30s-run1.json
//
// Clients are raw HTTP clients, not SDKs: what is measured is the server and
// the wire, and a raw client lets one generator hold 10 000 connections. They
// speak the SDK protocol (Bearer key, /stream or /flags/snapshot) and back off
// like the SDKs on refusal (3 s × 2^n, ≤ 30 s, +30 % jitter).
//
// SSE mode, in order:
//   ramp      open N streams (50 at a time per worker) until every one has
//             received `ready`, refusals retried as above
//   idle      --idle-s with no flag change: heartbeat cost
//   updates   --updates PATCHes of `hot`, one every --update-gap-s: delivery
//             traffic and time from PATCH sent to the event at every client
//   reconnect every stream dropped at once; each client reconnects after
//             3 s × (1 + up to 30 %), as an SDK does: until all are ready again
// Poll mode: every client GETs /flags/snapshot every --interval-s (start
//   offsets spread over one interval), --warmup-s unrecorded, then a window of
//   max(--window-s, 3 intervals) with one PATCH every --update-gap-s; for each
//   update, staleness = when each client first saw it − PATCH sent.
//
// The server's CPU and RSS come from /proc/<server-pid> and its network bytes
// (all of eth0: SDK traffic plus PostgreSQL and Redis) from
// /proc/<server-pid>/net/dev, read at the start and end of every window. This
// process must share the server's PID namespace (compose: pid: service:server).

import { fork } from 'node:child_process';
import http from 'node:http';
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { HOT_FLAG } from './fixtures.mjs';
import { parseArgs, summarize, procUsage, sleep, mib, round3 } from './util.mjs';

const now = () => performance.timeOrigin + performance.now();

// ── worker ──────────────────────────────────────────────────────────────────

if (process.argv[2] === '--worker') {
  const agent = new http.Agent({ keepAlive: true, maxSockets: Infinity });
  const clients = [];
  let cfg;
  const bytes = { bootstrap: 0, heartbeat: 0, update: 0, other: 0, poll: 0 };
  const seen = []; // [value, t] per hot-flag observation
  let refused = 0;
  let requests = 0;
  let errors = 0;
  const pollLatency = [];

  const backoff = (attempt) => {
    const base = Math.min(30000, 3000 * 2 ** attempt);
    return base + Math.random() * base * 0.3;
  };

  function openStream(c) {
    c.ready = false;
    c.phase = 'bootstrap';
    const req = http.get(`${cfg.url}/stream`, {
      agent: false, headers: { Authorization: `Bearer ${cfg.sdkKey}`, Accept: 'text/event-stream' },
    }, (res) => {
      if (res.statusCode !== 200) {
        refused++;
        res.resume();
        setTimeout(() => openStream(c), backoff(c.attempt++));
        return;
      }
      c.res = res;
      let buf = '';
      let event = '';
      let frameBytes = 0;
      res.setEncoding('utf8');
      res.on('data', (chunk) => {
        buf += chunk;
        let nl;
        while ((nl = buf.indexOf('\n')) >= 0) {
          const line = buf.slice(0, nl);
          buf = buf.slice(nl + 1);
          frameBytes += Buffer.byteLength(line) + 1;
          if (line.startsWith('event:')) event = line.slice(6).trim();
          else if (line.startsWith('data:') && event === 'update' && c.ready && line.includes(`"key":"${HOT_FLAG}"`)) {
            const m = line.match(/"default_value":"([^"]+)"/);
            if (m) seen.push([m[1], now()]);
          } else if (line === '') {
            const kind = !c.ready ? 'bootstrap' : event === 'heartbeat' ? 'heartbeat' : event === 'update' ? 'update' : 'other';
            bytes[kind] += frameBytes;
            frameBytes = 0;
            if (event === 'ready') { c.ready = true; c.attempt = 0; }
            event = '';
          }
        }
      });
      res.on('error', () => {});
      res.on('close', () => {
        c.res = null;
        c.ready = false;
        if (!c.dropped && !cfg.stopping) setTimeout(() => openStream(c), backoff(c.attempt++));
      });
    });
    req.on('error', () => {
      errors++;
      if (!cfg.stopping) setTimeout(() => openStream(c), backoff(c.attempt++));
    });
    c.req = req;
  }

  function pollOnce(c) {
    if (cfg.stopping) return;
    const t0 = now();
    requests++;
    http.get(`${cfg.url}/flags/snapshot`, { agent, headers: { Authorization: `Bearer ${cfg.sdkKey}` } }, (res) => {
      let body = '';
      res.setEncoding('utf8');
      res.on('data', (d) => { body += d; });
      res.on('end', () => {
        const t = now();
        bytes.poll += Buffer.byteLength(body);
        if (res.statusCode !== 200) { errors++; return; }
        pollLatency.push(t - t0);
        const m = body.match(new RegExp(`"key":"${HOT_FLAG}"[^}]*?"default_value":"([^"]+)"|"default_value":"([^"]+)"[^}]*?"key":"${HOT_FLAG}"`));
        const v = m && (m[1] || m[2]);
        if (v && v !== c.last) { c.last = v; seen.push([v, t]); }
      });
    }).on('error', () => { errors++; });
  }

  process.on('message', async (msg) => {
    if (msg.cmd === 'start') {
      cfg = msg;
      for (let i = 0; i < msg.count; i++) clients.push({ id: i, attempt: 0, ready: false, last: null });
      if (msg.mode === 'sse') {
        for (let i = 0; i < clients.length; i += 50) {
          clients.slice(i, i + 50).forEach(openStream);
          await sleep(20);
        }
      } else {
        for (const c of clients) {
          setTimeout(() => { pollOnce(c); c.timer = setInterval(() => pollOnce(c), msg.intervalMs); },
            Math.random() * msg.intervalMs);
        }
      }
    } else if (msg.cmd === 'stats') {
      process.send({
        cmd: 'stats', ready: clients.filter((c) => c.ready).length, bytes: { ...bytes },
        refused, requests, errors, seen: seen.splice(0), pollLatency: pollLatency.splice(0),
      });
    } else if (msg.cmd === 'drop') {
      // Every stream dropped at once; each reconnects like an SDK's first retry.
      for (const c of clients) {
        c.dropped = true;
        c.req && c.req.destroy();
        c.res && c.res.destroy();
        c.ready = false;
      }
      for (const c of clients) {
        c.dropped = false;
        c.attempt = 0;
        setTimeout(() => openStream(c), 3000 * (1 + Math.random() * 0.3));
      }
    } else if (msg.cmd === 'stop') {
      cfg.stopping = true;
      for (const c of clients) {
        clearInterval(c.timer);
        c.req && c.req.destroy();
        c.res && c.res.destroy();
      }
      process.exit(0);
    }
  });
} else {
  await main();
}

// ── coordinator ─────────────────────────────────────────────────────────────

async function main() {
  const args = parseArgs({
    session: '/work/session.json', mode: 'sse', clients: '1000', 'interval-s': '30',
    'idle-s': '60', updates: '30', 'update-gap-s': '2', 'warmup-s': '0', 'window-s': '60',
    'ramp-timeout-s': '300', 'clients-per-worker': '1250', 'max-workers': '8',
    'server-pid': '1', 'metrics-url': 'http://server:9090/metrics', out: '',
  });
  const session = JSON.parse(readFileSync(args.session, 'utf8'));
  const N = Number(args.clients);
  const mode = args.mode;
  const pid = Number(args['server-pid']);
  const workers = Math.min(Number(args['max-workers']), Math.max(1, Math.ceil(N / Number(args['clients-per-worker']))));

  const net = () => {
    try {
      const line = readFileSync(`/proc/${pid}/net/dev`, 'utf8').split('\n').find((l) => l.trim().startsWith('eth0:'));
      const f = line.split(':')[1].trim().split(/\s+/).map(Number);
      return { rx: f[0], tx: f[8] };
    } catch { return null; }
  };
  const metric = async (name) => {
    try {
      const text = await (await fetch(args['metrics-url'])).text();
      const m = text.match(new RegExp(`^${name}(?:\\{[^}]*\\})? ([0-9.e+]+)$`, 'm'));
      return m ? Number(m[1]) : null;
    } catch { return null; }
  };
  const snap = () => ({ t: now(), proc: procUsage(pid), net: net() });
  const delta = (a, b) => {
    const secs = (b.t - a.t) / 1000;
    return {
      secs: round3(secs),
      server_cpu_pct: a.proc && b.proc ? round3(((b.proc.cpu - a.proc.cpu) / secs) * 100) : null,
      server_rss_mib: b.proc ? mib(b.proc.rss) : null,
      server_rss_peak_mib: b.proc ? mib(b.proc.hwm) : null,
      net_rx_bytes: a.net && b.net ? b.net.rx - a.net.rx : null,
      net_tx_bytes: a.net && b.net ? b.net.tx - a.net.tx : null,
    };
  };

  const pool = [];
  for (let w = 0; w < workers; w++) {
    const count = Math.floor(N / workers) + (w < N % workers ? 1 : 0);
    const child = fork(fileURLToPath(import.meta.url), ['--worker'], { stdio: 'inherit' });
    pool.push({ child, count });
  }
  const stats = async () => {
    const parts = await Promise.all(pool.map(({ child }) => new Promise((resolve) => {
      child.once('message', resolve);
      child.send({ cmd: 'stats' });
    })));
    const sum = { ready: 0, refused: 0, requests: 0, errors: 0, bytes: {}, seen: [], pollLatency: [] };
    for (const p of parts) {
      sum.ready += p.ready; sum.refused += p.refused; sum.requests += p.requests; sum.errors += p.errors;
      for (const [k, v] of Object.entries(p.bytes)) sum.bytes[k] = (sum.bytes[k] || 0) + v;
      sum.seen.push(...p.seen); sum.pollLatency.push(...p.pollLatency);
    }
    return sum;
  };
  const bytesDelta = (a, b) => Object.fromEntries(Object.keys(b.bytes).map((k) => [k, b.bytes[k] - (a.bytes[k] || 0)]));

  let updateSeq = 0;
  const patch = async () => {
    const value = `c${Date.now().toString(36)}${updateSeq++}`;
    const sent = now();
    const res = await fetch(`${session.url}/api/environments/${session.environment_id}/flags/${HOT_FLAG}`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json', 'X-Checkgate-Request': 'true', Cookie: session.cookie },
      body: JSON.stringify({ default_value: value }),
    });
    if (res.status !== 200) throw new Error(`PATCH ${HOT_FLAG}: HTTP ${res.status}`);
    return { value, sent };
  };

  const result = {
    schema: 'checkgate-cost/v1', mode, clients: N, workers,
    interval_s: mode === 'poll' ? Number(args['interval-s']) : null, windows: {}, notes: [],
  };
  const start = { cmd: 'start', mode, url: session.url, sdkKey: session.sdk_key, intervalMs: Number(args['interval-s']) * 1000 };
  pool.forEach(({ child, count }) => child.send({ ...start, count }));

  try {
    if (mode === 'sse') {
      // ramp
      const r0 = snap();
      let s = await stats();
      const deadline = now() + Number(args['ramp-timeout-s']) * 1000;
      while (s.ready < N && now() < deadline) { await sleep(500); s = await stats(); }
      const r1 = snap();
      result.windows.ramp = {
        ...delta(r0, r1), ready: s.ready, all_ready: s.ready === N, refused_attempts: s.refused,
        bootstrap_bytes: s.bytes.bootstrap,
      };
      if (s.ready < N) throw new Error(`only ${s.ready}/${N} streams ready after ${args['ramp-timeout-s']} s`);
      result.windows.ramp.server_reported_connections = await metric('checkgate_sse_active_connections') ?? await metric('checkgate_sse_connections');

      // idle
      await sleep(2000);
      const i0 = snap(); const is0 = await stats();
      await sleep(Number(args['idle-s']) * 1000);
      const i1 = snap(); const is1 = await stats();
      const idleBytes = bytesDelta(is0, is1);
      result.windows.idle = {
        ...delta(i0, i1), client_bytes: idleBytes, ready: is1.ready,
        heartbeat_bytes_per_client_per_hour: round3((idleBytes.heartbeat / N) * (3600 / ((i1.t - i0.t) / 1000))),
      };

      // updates
      const u0 = snap(); const us0 = await stats();
      const sentAt = new Map();
      for (let k = 0; k < Number(args.updates); k++) {
        const { value, sent } = await patch();
        sentAt.set(value, sent);
        await sleep(Number(args['update-gap-s']) * 1000);
      }
      await sleep(3000);
      const u1 = snap(); const us1 = await stats();
      const lat = []; const perUpdate = new Map();
      for (const [v, t] of [...us0.seen, ...us1.seen]) {
        const sent = sentAt.get(v);
        if (sent === undefined) continue;
        lat.push(t - sent);
        perUpdate.set(v, (perUpdate.get(v) || 0) + 1);
      }
      const expected = Number(args.updates) * N;
      result.windows.updates = {
        ...delta(u0, u1), client_bytes: bytesDelta(us0, us1), updates: Number(args.updates),
        deliveries: lat.length, expected_deliveries: expected, missed: expected - lat.length,
        delivery_ms: summarize(lat),
        update_bytes_per_client_per_update: round3(bytesDelta(us0, us1).update / N / Number(args.updates)),
      };

      // reconnect
      const c0 = snap(); const cs0 = await stats();
      pool.forEach(({ child }) => child.send({ cmd: 'drop' }));
      await sleep(500);
      let cs = await stats();
      const cDeadline = now() + Number(args['ramp-timeout-s']) * 1000;
      while (cs.ready < N && now() < cDeadline) { await sleep(250); cs = await stats(); }
      const c1 = snap();
      result.windows.reconnect = {
        ...delta(c0, c1), ready: cs.ready, all_ready: cs.ready === N,
        refused_attempts: cs.refused - cs0.refused,
        rebootstrap_bytes: cs.bytes.bootstrap - cs0.bytes.bootstrap,
        rebootstrap_bytes_per_client: round3((cs.bytes.bootstrap - cs0.bytes.bootstrap) / N),
        note: 'includes the 3.0-3.9 s each client waits before reconnecting, as an SDK does',
      };
    } else {
      const interval = Number(args['interval-s']);
      await sleep(Math.max(Number(args['warmup-s']), interval) * 1000);
      const windowS = Math.max(Number(args['window-s']), interval * 3);
      const p0 = snap(); const ps0 = await stats();
      const sentAt = new Map();
      const gap = Number(args['update-gap-s']) * 1000;
      const end = now() + windowS * 1000;
      while (now() + gap < end) {
        const { value, sent } = await patch();
        sentAt.set(value, sent);
        await sleep(gap);
      }
      await sleep(Math.max(0, end - now()));
      const p1 = snap(); const ps1 = await stats();
      // Clients that saw an update after the window still count for staleness.
      await sleep(interval * 1000 + 2000);
      const ps2 = await stats();
      const stale = [];
      for (const [v, t] of [...ps0.seen, ...ps1.seen, ...ps2.seen]) {
        const sent = sentAt.get(v);
        if (sent !== undefined && t >= sent) stale.push(t - sent);
      }
      const secs = (p1.t - p0.t) / 1000;
      const req = ps1.requests - ps0.requests;
      const errs = ps1.errors - ps0.errors;
      result.windows.poll = {
        ...delta(p0, p1), requests: req, requests_per_s: round3(req / secs),
        target_requests_per_s: round3(N / interval), errors: errs,
        error_rate: round3(req ? errs / req : 0), client_bytes: bytesDelta(ps0, ps1),
        bytes_per_request: round3(req ? (ps1.bytes.poll - ps0.bytes.poll) / req : 0),
        request_ms: summarize(ps1.pollLatency), updates: sentAt.size, staleness_ms: summarize(stale),
      };
    }
  } catch (err) {
    result.error = String(err && err.message || err);
  } finally {
    pool.forEach(({ child }) => child.send({ cmd: 'stop' }));
  }
  if (args.out) writeFileSync(args.out, JSON.stringify(result, null, 2));
  process.stdout.write(JSON.stringify({ mode, clients: N, interval_s: result.interval_s, error: result.error || null }) + '\n');
  process.exit(result.error ? 1 : 0);
}
