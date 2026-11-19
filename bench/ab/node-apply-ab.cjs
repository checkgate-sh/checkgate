// A/B of configuration application cost in the Node SDK between two builds of
// its index.js: a complete set (stream replay on "ready", or a poll) and a
// live change event, with the native engine. Alternates the builds per round.
//
//   node bench/ab/node-apply-ab.cjs BASE/sdks/nodejs/index.js CAND/sdks/nodejs/index.js
//
// Cases, each for 100 and 1 000 flags:
//   replay-unchanged  a reconnect replay identical to the held set (the common case)
//   replay-changed    a replay in which every flag changed
//   poll-v2           a ?v=2 poll body with revisions (the candidate's new path;
//                     the base gets the same flags as an array, its only format)
//   event             one live UPSERT with a newer revision
// Prints one JSON line per (build, case, flags) with the median and p95 of
// ROUNDS rounds, each the mean over ITER applications.
const path = require('node:path');

const [baseEntry, candEntry] = process.argv.slice(2).map((p) => path.resolve(p));
if (!baseEntry || !candEntry) throw new Error('usage: node-apply-ab.cjs BASE_INDEX CAND_INDEX');
const ROUNDS = Number(process.env.ROUNDS || 15);
for (const level of ['log', 'warn', 'error']) console[level] = () => {};

function flagSet(n, variant) {
    return Array.from({ length: n }, (_, i) => ({
        key: `flag-${i}`, is_enabled: (i + variant) % 3 !== 0, rollout_percentage: (i * 7 + variant) % 101,
        description: `v${variant}`, flag_type: 'boolean',
        rules: [{ attribute: 'plan', operator: 'equals', values: ['pro', `p${variant}`] }],
    }));
}

/** A client with `flags` applied through a replay at revisions base+i. */
function loaded(Client, flags, base) {
    const c = new Client({ serverUrl: 'http://127.0.0.1:9', sdkKey: 'sk', reportImpressions: false });
    replay(c, flags, base);
    return c;
}

/** Drives a stream replay through the client's own handlers. */
function replay(c, flags, base) {
    if (c._connectDeltas) {
        c._streamStartedAt = c._learnSeq; // what opening a new stream records (no-op for the base)
    }
    c._staged = new Map();
    c._stagedRevisions = new Map();
    flags.forEach((f, i) => c._applyDelta({ type: 'UPSERT', flag: f, revision: base + i }));
    const staged = c._staged, revisions = c._stagedRevisions;
    c._staged = null; c._stagedRevisions = null;
    c._applySnapshot(c._learnSeq === undefined ? Array.from(staged.values()) : staged, revisions, c._streamStartedAt);
}

function timeIt(iter, fn) {
    const t0 = process.hrtime.bigint();
    for (let i = 0; i < iter; i++) fn(i);
    return Number(process.hrtime.bigint() - t0) / iter / 1000; // µs
}

function caseFns(Client, isCand, n) {
    const a = flagSet(n, 1), b = flagSet(n, 2);
    return {
        'replay-unchanged': () => {
            const c = loaded(Client, a, 1);
            return () => replay(c, a, 1);
        },
        'replay-changed': () => {
            const c = loaded(Client, a, 1);
            let flip = false, rev = 1;
            return () => { flip = !flip; rev += n; replay(c, flip ? b : a, rev); };
        },
        'poll-v2': () => {
            const c = loaded(Client, a, 1);
            let flip = false, rev = 1;
            return () => {
                flip = !flip; rev += n;
                const flags = flip ? b : a;
                if (isCand) {
                    const revisions = new Map(flags.map((f, i) => [f.key, { revision: rev + i, deleted: false }]));
                    c._applySnapshot(flags, revisions, c._learnSeq);
                } else {
                    c._applySnapshot(flags);
                }
            };
        },
        event: () => {
            const c = loaded(Client, a, 1);
            let rev = 10 * n;
            return () => { rev++; c._applyDelta({ type: 'UPSERT', flag: rev % 2 ? a[0] : b[0], revision: rev }); };
        },
    };
}

const builds = [['base', require(baseEntry).CheckgateClient], ['cand', require(candEntry).CheckgateClient]];
const results = new Map();
for (let round = 0; round < ROUNDS; round++) {
    for (const [label, Client] of round % 2 ? [...builds].reverse() : builds) {
        for (const n of [100, 1000]) {
            const cases = caseFns(Client, label === 'cand', n);
            for (const [name, make] of Object.entries(cases)) {
                const fn = make();
                const iter = name === 'event' ? 20000 : (n === 100 ? 400 : 40);
                timeIt(Math.ceil(iter / 10), fn); // warm-up
                const us = timeIt(iter, fn);
                const id = `${label}|${name}|${n}`;
                if (!results.has(id)) results.set(id, []);
                results.get(id).push(us);
            }
        }
    }
}
const q = (xs, p) => { const s = [...xs].sort((x, y) => x - y); return s[Math.min(s.length - 1, Math.round((s.length - 1) * p))]; };
for (const [id, xs] of results) {
    const [build, name, flags] = id.split('|');
    process.stdout.write(JSON.stringify({ build, case: name, flags: Number(flags), rounds: xs.length,
        median_us: Number(q(xs, 0.5).toFixed(2)), p95_us: Number(q(xs, 0.95).toFixed(2)) }) + '\n');
}
