// Raw SSE delivery audit for scripts/release-benchmarks/v0.2.0-linux.sh.
//
//   node sse_audit.mjs --url http://127.0.0.1:3901 --sdk-key KEY \
//       --streams 25 --stop-file /path/stop --out audit.json
//
// Opens --streams plain HTTP connections to /stream (no SDK, no reconnect)
// and parses every SSE frame. After a stream's `ready` event, each `update`
// is checked against what that stream already received:
//
//   duplicated   — the same (key, revision) delivered twice on one stream
//   out_of_order — a revision lower than one already delivered for the key
//   disconnects  — a stream that ended or errored before the audit stopped
//
// The SDK-level propagation harness (bench/public/sdk/propagation.mjs) sees
// values, not deliveries: a duplicate leaves the value unchanged and is
// invisible there. Runs until --stop-file exists or --max-secs elapse.

import http from 'node:http';
import { existsSync, writeFileSync } from 'node:fs';

const args = { url: '', 'sdk-key': '', streams: '25', 'stop-file': '', 'max-secs': '3600', out: '' };
for (let i = 2; i < process.argv.length; i += 2) {
  const k = process.argv[i].replace(/^--/, '');
  if (!(k in args)) throw new Error(`unknown flag --${k}`);
  args[k] = process.argv[i + 1];
}
const target = new URL('/stream', args.url);
const streams = Number(args.streams);
const started = Date.now();
const totals = {
  streams,
  connected: 0,
  ready: 0,
  updates_after_ready: 0,
  duplicated: 0,
  out_of_order: 0,
  disconnects: 0,
  http_errors: 0,
  heartbeats: 0,
  examples: [],
};
const live = new Set();
let stopping = false;

function note(kind, detail) {
  if (totals.examples.length < 20) totals.examples.push({ kind, ...detail });
}

function open(id) {
  const seen = new Map(); // key -> Set(revision)
  const last = new Map(); // key -> highest revision
  let ready = false;
  let buf = '';
  const req = http.get(target, { headers: { Authorization: `Bearer ${args['sdk-key']}` } }, (res) => {
    if (res.statusCode !== 200) {
      totals.http_errors++;
      res.resume();
      return;
    }
    totals.connected++;
    live.add(req);
    res.setEncoding('utf8');
    res.on('data', (chunk) => {
      buf += chunk;
      let idx;
      while ((idx = buf.indexOf('\n\n')) >= 0) {
        const frame = buf.slice(0, idx);
        buf = buf.slice(idx + 2);
        let event = 'message';
        let data = '';
        for (const line of frame.split('\n')) {
          if (line.startsWith('event:')) event = line.slice(6).trim();
          else if (line.startsWith('data:')) data += line.slice(5).trim();
        }
        if (event === 'ready') {
          ready = true;
          totals.ready++;
        } else if (event === 'heartbeat') {
          totals.heartbeats++;
        } else if (event === 'update') {
          let msg;
          try {
            msg = JSON.parse(data);
          } catch {
            continue;
          }
          const key = msg.flag?.key ?? msg.key;
          const rev = msg.revision;
          if (key === undefined || rev === undefined) continue;
          const revs = seen.get(key) ?? new Set();
          if (ready) {
            totals.updates_after_ready++;
            if (revs.has(rev)) {
              totals.duplicated++;
              note('duplicated', { stream: id, key, revision: rev });
            } else if ((last.get(key) ?? -Infinity) > rev) {
              totals.out_of_order++;
              note('out_of_order', { stream: id, key, revision: rev, after: last.get(key) });
            }
          }
          revs.add(rev);
          seen.set(key, revs);
          last.set(key, Math.max(last.get(key) ?? -Infinity, rev));
        }
      }
    });
    res.on('end', () => {
      live.delete(req);
      if (!stopping) {
        totals.disconnects++;
        note('disconnect', { stream: id, after_s: (Date.now() - started) / 1000 });
      }
    });
  });
  req.on('error', (e) => {
    live.delete(req);
    if (!stopping) {
      totals.disconnects++;
      note('error', { stream: id, message: e.message });
    }
  });
}

for (let i = 0; i < streams; i++) open(i);

const timer = setInterval(() => {
  const elapsed = (Date.now() - started) / 1000;
  if ((args['stop-file'] && existsSync(args['stop-file'])) || elapsed > Number(args['max-secs'])) {
    stopping = true;
    clearInterval(timer);
    for (const r of live) r.destroy();
    totals.seconds = Math.round(elapsed * 1000) / 1000;
    const text = JSON.stringify(totals, null, 2);
    if (args.out) writeFileSync(args.out, text);
    else console.log(text);
    process.exit(0);
  }
}, 200);
