// Runs the shared fault-injection suite against the Flutter SDK's Dart client,
// through tool/conformance_adapter.dart (one adapter process per scenario).
//
//   node --test sdks/conformance/flutter.mjs                       # CI seeds
//   CONFORMANCE_SEEDS=1-500 node --test sdks/conformance/flutter.mjs   # extended
//   CHECKGATE_FLUTTER_LIB=target/debug/libflutter.dylib …          # real engine
//
// Needs `flutter pub get` in sdks/flutter/dart first. DART defaults to `dart`.

import { test } from 'node:test';
import { spawn } from 'node:child_process';
import readline from 'node:readline';
import { fileURLToPath } from 'node:url';
import { CI_SEEDS, NAMED, generateScenario, runScenario, seedsFromEnv } from './scenario.mjs';

const dartDir = fileURLToPath(new URL('../flutter/dart/', import.meta.url));

function startAdapter() {
    const proc = spawn(process.env.DART || 'dart', ['run', 'tool/conformance_adapter.dart'], {
        cwd: dartDir, stdio: ['pipe', 'pipe', 'inherit'],
    });
    const waiting = new Map();
    let next = 1;
    readline.createInterface({ input: proc.stdout }).on('line', (line) => {
        let msg;
        try { msg = JSON.parse(line); } catch { return; }
        const w = waiting.get(msg.id);
        if (w) { waiting.delete(msg.id); w(msg); }
    });
    const call = (cmd, extra = {}) => new Promise((resolve) => {
        const id = next++;
        waiting.set(id, resolve);
        proc.stdin.write(JSON.stringify({ id, cmd, ...extra }) + '\n');
    });
    return { proc, call };
}

const adapter = {
    async create(url, key, timing) {
        const a = startAdapter();
        await a.call('create', { url, key, timing });
        return a;
    },
    async connect(a) {
        const r = await a.call('connect');
        if (!r.ok) throw new Error(r.error);
    },
    async flags(a) { return new Map(Object.entries((await a.call('flags')).flags)); },
    async evaluate(a, key, user) {
        const v = (await a.call('evaluate', { key, user })).value;
        return v === null ? undefined : v;
    },
    async status(a) { return { state: (await a.call('status')).state }; },
    async sample(a, users) {
        const r = await a.call('sample', { users });
        return { flags: new Map(Object.entries(r.flags)), evals: new Map(Object.entries(r.evals)) };
    },
    async close(a) {
        await a.call('close');
        await a.call('exit');
        a.proc.kill();
    },
};

const TIMEOUT = 60_000;
for (const [name, scenario] of Object.entries(NAMED)) {
    test(`[flutter] ${name}`, { timeout: TIMEOUT }, async () => {
        await runScenario(adapter, { seed: 0, ...scenario }, { label: `flutter-${name}` });
    });
}
for (const seed of seedsFromEnv(CI_SEEDS)) {
    test(`[flutter] random seed ${seed}`, { timeout: TIMEOUT }, async () => {
        await runScenario(adapter, generateScenario(seed), { label: `flutter-seed-${seed}` });
    });
}
