// Randomized, reproducible SSE fault scenarios and the invariants every SDK
// must keep under them. Shared by all SDK suites; see README.md.
//
// A scenario is a list of steps generated from a seed (mulberry32, so a seed
// means the same steps in every language and on every machine). The runner
// drives a FaultServer, samples the client every SAMPLE_MS, and checks:
//
//   I1 every flag version the client serves existed on the server (no partial
//      or corrupt state);
//   I2 per flag, the served revision never goes back (no out-of-order
//      regression), and a deleted flag only comes back at a newer revision;
//   I3 a flag disappears only after the server deleted it;
//   I4 while a flag's version is unchanged, its evaluation is unchanged for
//      the same user (no flapping from reconnects alone);
//   I5 the client never holds more than MAX_STREAMS streams at once, and has
//      exactly one open stream once it has recovered (no leaked connections);
//   I6 connect() settles: resolves, or rejects with an auth error;
//   I7 once faults heal, the client serves exactly the server's flag set
//      within the convergence bound (no stale flags indefinitely).
//
// A failure reports the seed and the full step trace, and writes both to
// $CONFORMANCE_TRACE_DIR when it is set.

import fs from 'node:fs';
import path from 'node:path';
import { FaultServer } from './fault-server.mjs';

/** Client timing used by every suite: fast, but the same rules as production. */
export const TIMING = {
    heartbeatMs: 100,          // server heartbeat; idle deadline = 3 × = 300 ms
    reconnectDelayMs: 20,
    maxReconnectDelayMs: 200,
    pollFallbackThreshold: 3,
    pollIntervalMs: 100,
    connectTimeoutMs: 500,
};

/**
 * After the last fault heals, a client must converge within: one idle
 * deadline (to notice a silent stream) + one connect deadline + the largest
 * backoff with jitter + a scheduling margin for loaded CI machines.
 */
export const CONVERGE_MS = TIMING.heartbeatMs * 3 + TIMING.connectTimeoutMs
    + TIMING.maxReconnectDelayMs * 1.3 + 2000;

const SAMPLE_MS = 25;
/** A replaced stream may still be closing on the server when the next opens. */
const MAX_STREAMS = 2;
const USERS = ['user-1', 'user-2', 'user-3'];
const KEYS = ['alpha', 'beta', 'gamma', 'delta', 'epsilon', 'zeta'];

export function mulberry32(seed) {
    let a = seed >>> 0;
    return () => {
        a = (a + 0x6d2b79f5) >>> 0;
        let t = a;
        t = Math.imul(t ^ (t >>> 15), t | 1);
        t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
        return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
}

function randomFlag(rand, key) {
    const flag = {
        key,
        is_enabled: rand() < 0.7,
        rollout_percentage: rand() < 0.5 ? Math.floor(rand() * 101) : null,
        rules: [],
    };
    if (rand() < 0.4) {
        flag.rules.push({ attribute: 'plan', operator: 'equals', values: ['pro'] });
    }
    return flag;
}

const FAULTS = [
    'sever', 'stall', 'truncate', 'cutReplay', 'duplicate', 'stale',
    'fragmentOn', 'fragmentOff', 'delayOn', 'delayOff',
    'restart', 'unavailable', 'slowSnapshot',
];

/**
 * Steps for one seed. `ops` is how many steps; `withAuth` lets the scenario
 * end with a key revocation and recovery (the default: one seed in four).
 */
export function generateScenario(seed, { ops = 40, withAuth } = {}) {
    const rand = mulberry32(seed);
    const steps = [];
    const initial = KEYS.slice(0, 3 + Math.floor(rand() * 3)).map((k) => randomFlag(rand, k));
    for (let i = 0; i < ops; i++) {
        const r = rand();
        if (r < 0.35) {
            const key = KEYS[Math.floor(rand() * KEYS.length)];
            steps.push({ op: 'upsert', flag: randomFlag(rand, key) });
        } else if (r < 0.45) {
            steps.push({ op: 'delete', key: KEYS[Math.floor(rand() * KEYS.length)] });
        } else if (r < 0.50) {
            // A burst of rapid updates to one flag.
            const key = KEYS[Math.floor(rand() * KEYS.length)];
            for (let j = 0; j < 5; j++) steps.push({ op: 'upsert', flag: randomFlag(rand, key) });
        } else if (r < 0.75) {
            steps.push({ op: 'fault', kind: FAULTS[Math.floor(rand() * FAULTS.length)] });
        } else {
            steps.push({ op: 'wait', ms: 10 + Math.floor(rand() * 250) });
        }
    }
    const auth = withAuth ?? rand() < 0.25;
    if (auth) steps.push({ op: 'revoke' }, { op: 'wait', ms: 600 }, { op: 'restore' });
    return { seed, initial, steps };
}

/**
 * Named regression scenarios: the specific failure modes from the brief, each
 * as a fixed step list so every SDK runs exactly the same thing.
 */
export const NAMED = {
    'half-open stream after ready is detected': {
        initial: [{ key: 'alpha', is_enabled: true, rules: [] }],
        steps: [{ op: 'wait', ms: 200 }, { op: 'mark', name: 'stall' }, { op: 'fault', kind: 'stall' },
            { op: 'upsert', flag: { key: 'alpha', is_enabled: false, rules: [] } }],
        expect: { detectStallWithinMs: TIMING.heartbeatMs * 3 + 400 },
    },
    'a quiet but healthy stream is not reconnected': {
        initial: [{ key: 'alpha', is_enabled: true, rules: [] }],
        steps: [{ op: 'wait', ms: TIMING.heartbeatMs * 3 * 5 }],
        expect: { streamsOpened: 1 },
    },
    'a quiet stream from a pre-0.1.31 server is not reconnected': {
        // No `heartbeat_ms`, keep-alive comments only: an SDK must not mistake
        // the old server's quiet stream for a dead one.
        server: { announceHeartbeat: false },
        initial: [{ key: 'alpha', is_enabled: true, rules: [] }],
        steps: [{ op: 'wait', ms: TIMING.heartbeatMs * 3 * 5 }],
        expect: { streamsOpened: 1 },
    },
    'replay cut mid-frame keeps the previous snapshot': {
        initial: [{ key: 'alpha', is_enabled: true, rules: [] }, { key: 'beta', is_enabled: true, rules: [] },
            { key: 'gamma', is_enabled: false, rules: [] }],
        steps: [{ op: 'fault', kind: 'cutReplay' }, { op: 'upsert', flag: { key: 'beta', is_enabled: false, rules: [] } },
            { op: 'fault', kind: 'sever' }, { op: 'wait', ms: 400 }],
    },
    'event cut mid-frame is not applied': {
        initial: [{ key: 'alpha', is_enabled: true, rules: [] }],
        steps: [{ op: 'fault', kind: 'truncate' }, { op: 'upsert', flag: { key: 'alpha', is_enabled: false, rules: [] } },
            { op: 'wait', ms: 400 }],
    },
    'older event after reconnect is ignored': {
        initial: [{ key: 'alpha', is_enabled: true, rules: [] }],
        steps: [{ op: 'upsert', flag: { key: 'alpha', is_enabled: false, rules: [] } }, { op: 'wait', ms: 100 },
            { op: 'fault', kind: 'stale' }, { op: 'fault', kind: 'sever' }, { op: 'wait', ms: 400 }],
    },
    'duplicate events are harmless': {
        initial: [{ key: 'alpha', is_enabled: true, rules: [] }],
        steps: [{ op: 'fault', kind: 'duplicate' }, { op: 'upsert', flag: { key: 'alpha', is_enabled: false, rules: [] } },
            { op: 'fault', kind: 'duplicate' }, { op: 'delete', key: 'alpha' }, { op: 'wait', ms: 200 }],
    },
    'server restart during an update burst': {
        initial: [{ key: 'alpha', is_enabled: true, rules: [] }],
        steps: [{ op: 'upsert', flag: { key: 'alpha', is_enabled: false, rules: [] } }, { op: 'fault', kind: 'restart' },
            { op: 'upsert', flag: { key: 'beta', is_enabled: true, rules: [] } },
            { op: 'upsert', flag: { key: 'alpha', is_enabled: true, rules: [] } }, { op: 'wait', ms: 500 }],
    },
    'slow poll cannot roll back the stream': {
        initial: [{ key: 'alpha', is_enabled: true, rules: [] }],
        steps: [{ op: 'fault', kind: 'slowSnapshot' }, { op: 'fault', kind: 'unavailable' },
            { op: 'upsert', flag: { key: 'alpha', is_enabled: false, rules: [] } }, { op: 'wait', ms: 300 }],
    },
    'revoked key stops retrying and recovers on connect()': {
        initial: [{ key: 'alpha', is_enabled: true, rules: [] }],
        steps: [{ op: 'revoke' }, { op: 'wait', ms: 600 }, { op: 'restore' }],
        expect: { authFailure: true },
    },
};

/**
 * The seam to one SDK. Every method may be async.
 * - create(url, key, timing) → client          - connect(client) → Promise
 * - flags(client) → Map key → flag JSON         - status(client) → { state }
 * - evaluate(client, key, user) → boolean | undefined (undefined: no engine)
 * - close(client)
 * - sample(client, users) → { flags, evals: Map "key/user" → value } (optional):
 *   flags and evaluations read at one instant, for clients in another process
 *   where an event could otherwise land between the two reads
 */

async function readSample(adapter, client) {
    if (adapter.sample) return adapter.sample(client, USERS);
    // In-process clients: no event can be handled between these awaits,
    // which all resolve synchronously.
    const flags = await adapter.flags(client);
    const evals = new Map();
    for (const key of flags.keys()) {
        for (const user of USERS) evals.set(`${key}/${user}`, await adapter.evaluate(client, key, user));
    }
    return { flags, evals };
}

class Violation extends Error {}

export async function runScenario(adapter, scenario, { label = String(scenario.seed), log = () => {} } = {}) {
    const server = new FaultServer({ heartbeatMs: TIMING.heartbeatMs, ...(scenario.server || {}) });
    const url = await server.start();
    const trace = [];
    const t0 = Date.now();
    const note = (what) => trace.push({ t: Date.now() - t0, ...what });
    const violations = [];
    const fail = (msg) => {
        violations.push(msg);
        note({ violation: msg });
    };
    for (const flag of scenario.initial) server.upsert(flag);

    // Per key: { revision, deleted } last served, and per (key,user) the last
    // evaluation with the revision it was made at.
    const served = new Map();
    const evals = new Map();
    let client;
    let sampling = true;
    let samplerError = null;
    const timers = new Set();
    const later = (ms, fn) => {
        const t = setTimeout(() => { timers.delete(t); fn(); }, ms);
        timers.add(t);
    };
    const result = { label, seed: scenario.seed, recoveryMs: null, detectMs: null, counters: null, violations, trace };

    async function sample() {
        const { flags, evals: values } = await readSample(adapter, client);
        const seen = new Set();
        for (const [key, flag] of flags) {
            seen.add(key);
            const rev = Number(String(flag && flag.description || '').replace('rev:', ''));
            const history = server.history.get(key);
            const original = history && history.get(rev);
            if (!original || JSON.stringify(original) !== JSON.stringify(flag)) {
                fail(`I1 ${key}: serves a version the server never had: ${JSON.stringify(flag)}`);
                continue;
            }
            const prev = served.get(key);
            if (prev && rev < prev.revision) fail(`I2 ${key}: went back from rev ${prev.revision} to ${rev}`);
            if (prev && prev.deleted && rev <= prev.revision) fail(`I2 ${key}: resurrected at rev ${rev} after delete rev ${prev.revision}`);
            if (!prev || rev !== prev.revision || prev.deleted) served.set(key, { revision: rev, deleted: false });
            for (const user of USERS) {
                const value = values.get(`${key}/${user}`);
                if (value === undefined || value === null) continue;
                const id = `${key}/${user}`;
                const last = evals.get(id);
                if (last && last.revision === rev && last.value !== value) {
                    fail(`I4 ${key} for ${user}: ${last.value} → ${value} at unchanged rev ${rev}`);
                }
                evals.set(id, { revision: rev, value });
            }
        }
        for (const [key, prev] of served) {
            if (prev.deleted || seen.has(key)) continue;
            // Gone: only legitimate after a server delete newer than what was served.
            const history = server.history.get(key) || new Map();
            const del = [...history].find(([r, f]) => f === null && r > prev.revision);
            if (!del) fail(`I3 ${key}: disappeared at rev ${prev.revision} without a newer delete`);
            served.set(key, { revision: del ? del[0] : prev.revision, deleted: true });
        }
        if (server.streams.size > MAX_STREAMS) fail(`I5 ${server.streams.size} streams open at once`);
    }

    const sampler = (async () => {
        while (sampling) {
            try {
                if (client) await sample();
            } catch (err) {
                samplerError = err;
                return;
            }
            await new Promise((r) => setTimeout(r, SAMPLE_MS));
        }
    })();

    async function settle(promise, ms, what) {
        let timer;
        const outcome = await Promise.race([
            promise.then(() => 'resolved', (err) => err),
            new Promise((r) => { timer = setTimeout(() => r('timeout'), ms); }),
        ]);
        clearTimeout(timer);
        if (outcome === 'timeout') fail(`I6 ${what} did not settle within ${ms} ms`);
        return outcome;
    }

    const heal = () => {
        server.mode = 'pass';
        server.fragment = false;
        server.writeDelayMs = 0;
        server.snapshotDelayMs = 0;
        server.setAcceptedKey(server.sdkKey);
    };

    try {
        client = await adapter.create(url, server.sdkKey, TIMING);
        const first = await settle(adapter.connect(client), CONVERGE_MS, 'connect()');
        note({ connect: String(first) });
        let stallAt = null;
        let authSeen = false;

        for (const step of scenario.steps) {
            note({ step });
            switch (step.op) {
                case 'upsert': server.upsert(step.flag); break;
                case 'delete': server.delete(step.key); break;
                case 'wait': await new Promise((r) => setTimeout(r, step.ms)); break;
                case 'mark': if (step.name === 'stall') stallAt = Date.now(); break;
                case 'revoke': {
                    server.setAcceptedKey(null);
                    server.severAll();
                    break;
                }
                case 'restore': {
                    const status = await adapter.status(client);
                    if (status.state !== 'auth_failed') {
                        fail(`auth: expected state auth_failed after revocation, got ${status.state}`);
                    } else {
                        authSeen = true;
                        // Terminal: no retries while the key stays rejected.
                        const before = server.counters.streamRequests + server.counters.snapshotRequests;
                        await new Promise((r) => setTimeout(r, 400));
                        const after = server.counters.streamRequests + server.counters.snapshotRequests;
                        if (after !== before) fail(`auth: ${after - before} requests after the key was rejected`);
                    }
                    server.setAcceptedKey(server.sdkKey);
                    const again = await settle(adapter.connect(client), CONVERGE_MS, 'connect() after restore');
                    note({ reconnect: String(again) });
                    break;
                }
                case 'fault': {
                    switch (step.kind) {
                        case 'sever': server.severAll(); break;
                        case 'stall': server.stallAll(); break;
                        case 'truncate': server.truncateNext(); break;
                        case 'cutReplay': server.cutNextReplay(); break;
                        case 'duplicate': server.duplicateNext(); break;
                        case 'stale': server.staleAfterNextReady(); break;
                        case 'fragmentOn': server.fragment = true; break;
                        case 'fragmentOff': server.fragment = false; break;
                        case 'delayOn': server.writeDelayMs = 30; break;
                        case 'delayOff': server.writeDelayMs = 0; break;
                        case 'slowSnapshot': server.snapshotDelayMs = 400; break;
                        case 'restart':
                            server.mode = 'refuse';
                            server.severAll();
                            later(150, () => { if (server.mode === 'refuse') server.mode = 'pass'; });
                            break;
                        case 'unavailable':
                            server.mode = 'unavailable';
                            server.severAll();
                            later(250, () => { if (server.mode === 'unavailable') server.mode = 'pass'; });
                            break;
                        default: throw new Error(`unknown fault ${step.kind}`);
                    }
                    break;
                }
                default: throw new Error(`unknown step ${step.op}`);
            }
        }

        // Heal and measure convergence (I7).
        for (const t of timers) clearTimeout(t);
        timers.clear();
        heal();
        const healedAt = Date.now();
        note({ healed: true });
        const want = server.authoritative();
        let converged = false;
        while (Date.now() - healedAt < CONVERGE_MS) {
            const have = await adapter.flags(client);
            if (have.size === want.size && [...want].every(([k, f]) => JSON.stringify(have.get(k)) === JSON.stringify(f))
                && server.streams.size === 1) {
                converged = true;
                break;
            }
            await new Promise((r) => setTimeout(r, 10));
        }
        result.recoveryMs = Date.now() - healedAt;
        if (!converged) {
            const have = await adapter.flags(client);
            const rev = (f) => (f ? f.description : 'absent');
            const diff = [...new Set([...have.keys(), ...want.keys()])]
                .filter((k) => JSON.stringify(have.get(k)) !== JSON.stringify(want.get(k)))
                .map((k) => `${k}: client ${rev(have.get(k))}, server ${rev(want.get(k))}`);
            fail(`I7 not converged ${CONVERGE_MS} ms after healing (stale flags): ${diff.join('; ') || 'flags match'}; ` +
                `${server.streams.size} open streams`);
        }
        if (stallAt !== null) {
            const next = server.streamOpenedAt.find((t) => t > stallAt);
            result.detectMs = next ? next - stallAt : null;
        }
        const expect = scenario.expect || {};
        if (expect.detectStallWithinMs !== undefined
            && (result.detectMs === null || result.detectMs > expect.detectStallWithinMs)) {
            fail(`silent stream detected after ${result.detectMs} ms, bound ${expect.detectStallWithinMs} ms`);
        }
        if (expect.streamsOpened !== undefined && server.counters.streamsOpened !== expect.streamsOpened) {
            fail(`a quiet healthy stream was reconnected: ${server.counters.streamsOpened} streams opened`);
        }
        if (expect.authFailure && !authSeen) fail('auth: the revoked key was never reported');
    } finally {
        sampling = false;
        await sampler;
        if (samplerError) fail(`sampler: ${samplerError && samplerError.stack}`);
        for (const t of timers) clearTimeout(t);
        if (client) await adapter.close(client);
        // I5 after close: every stream released.
        const deadline = Date.now() + 1000;
        while (server.streams.size > 0 && Date.now() < deadline) await new Promise((r) => setTimeout(r, 10));
        if (server.streams.size > 0) fail(`I5 ${server.streams.size} streams still open after close`);
        result.counters = { ...server.counters };
        await server.stop();
    }

    if (violations.length > 0) {
        const dir = process.env.CONFORMANCE_TRACE_DIR;
        if (dir) {
            fs.mkdirSync(dir, { recursive: true });
            fs.writeFileSync(path.join(dir, `${label.replace(/[^\w.-]+/g, '_')}.json`),
                JSON.stringify({ label, seed: scenario.seed, scenario, violations, trace }, null, 2));
        }
        log(JSON.stringify({ label, violations, trace }, null, 2));
        throw new Violation(`[${label}] seed ${scenario.seed}: ${violations.length} violation(s): ${violations.slice(0, 5).join(' | ')}`);
    }
    if (process.env.CONFORMANCE_REPORT) {
        fs.appendFileSync(process.env.CONFORMANCE_REPORT, JSON.stringify({
            label, seed: scenario.seed, steps: scenario.steps.length,
            recoveryMs: result.recoveryMs, detectMs: result.detectMs, counters: result.counters,
        }) + '\n');
    }
    return result;
}

/** Seeds to run: $CONFORMANCE_SEEDS ("1,2,3" or "1-200"), else the CI set. */
export function seedsFromEnv(ciSeeds) {
    const raw = process.env.CONFORMANCE_SEEDS;
    if (!raw) return ciSeeds;
    return raw.split(',').flatMap((part) => {
        const [a, b] = part.split('-').map(Number);
        return b === undefined ? [a] : Array.from({ length: b - a + 1 }, (_, i) => a + i);
    });
}

/** The seeds every pull request runs, including past failures (see README.md). */
export const CI_SEEDS = JSON.parse(fs.readFileSync(new URL('./ci-seeds.json', import.meta.url), 'utf8')).seeds;
