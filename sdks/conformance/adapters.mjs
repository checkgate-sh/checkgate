// Live adapters: each SDK's real client with its real transport and a real
// Rust engine, for the end-to-end suite (live.mjs). Nothing is mocked except
// where a platform cannot run on a host, and that is named:
//
// - node:          @checkgate/node, eventsource, native addon (napi)
// - web:           @checkgate/web, eventsource made to hide HTTP status like a
//                  browser, WASM engine (sdks/web/dist, `npm run build`)
// - react-native:  @checkgate/react-native, react-native-sse over a streaming
//                  XHR like React Native's; the JSI bridge is replaced by the
//                  same Rust engine compiled to WASM (JSI exists only on a device)
// - flutter:       checkgate_flutter's Dart client and SSE loop, Rust engine via
//                  FFI from a host build (CHECKGATE_FLUTTER_LIB)

import { createRequire } from 'node:module';
import { spawn } from 'node:child_process';
import readline from 'node:readline';
import { fileURLToPath } from 'node:url';

const here = (p) => fileURLToPath(new URL(p, import.meta.url));
const requireFrom = (dir) => createRequire(here(`${dir}/package.json`));

function quiet() {
    for (const level of ['log', 'warn', 'error']) console[level] = () => {};
}

function options(url, sdkKey, t) {
    return {
        serverUrl: url, sdkKey, reportImpressions: false,
        reconnectDelayMs: t.reconnectDelayMs, maxReconnectDelayMs: t.maxReconnectDelayMs,
        pollFallbackThreshold: t.pollFallbackThreshold, pollIntervalMs: t.pollIntervalMs,
        connectTimeoutMs: t.connectTimeoutMs,
    };
}

/** Methods shared by the in-process JS clients. */
const jsClient = (Client, extra = {}) => ({
    create: (url, key, t) => new Client({ ...options(url, key, t), ...extra }),
    connect: (c) => c.connect(),
    flags: (c) => new Map(c._flagCache),
    evaluate: (c, key, user, attrs = {}) => c.isEnabled(key, user, attrs),
    value: (c, key, user) => c.getValue(key, user, {}, null),
    status: (c) => c.getStatus(),
    close: (c) => c.disconnect(),
});

async function browserLikeEventSource() {
    const { EventSource } = requireFrom('../web')('eventsource');
    return class BrowserLikeEventSource extends EventSource {
        addEventListener(type, listener, opts) {
            if (type !== 'error') return super.addEventListener(type, listener, opts);
            return super.addEventListener('error', () => listener({ type: 'error' }), opts);
        }
    };
}

/** The WASM engine with the JSI bridge's method names (camelCase, objects in, JSON out). */
async function wasmBridge() {
    const wasm = await import(here('../web/dist/checkgate.js'));
    const fs = await import('node:fs');
    await wasm.default({ module_or_path: fs.readFileSync(here('../web/dist/checkgate_bg.wasm')) });
    return () => {
        const core = new wasm.CheckgateCoreWasm();
        return {
            upsertFlagV2: (flag) => core.upsert_flag_v2(JSON.stringify(flag)),
            deleteFlag: (key) => core.delete_flag(key),
            clearStore: () => core.clear_store(),
            isEnabled: (key, user, attrs) => core.is_enabled(key, user, attrs || {}),
            getVariant: (key, user, attrs) => JSON.stringify(core.get_variant(key, user, attrs || {}) ?? null),
        };
    };
}

export async function nodeAdapter() {
    quiet();
    // CONFORMANCE_NODE_ENTRY runs the suite against another build of the
    // client (e.g. a previous release, to show which checks it fails).
    const { CheckgateClient } = requireFrom('../nodejs')(process.env.CONFORMANCE_NODE_ENTRY || './index.js');
    return { name: 'node', engine: 'native addon', ...jsClient(CheckgateClient) };
}

export async function webAdapter() {
    quiet();
    globalThis.EventSource = await browserLikeEventSource();
    // The SDK's own init() fetches the .wasm by URL; give it the bytes.
    const wasm = await import(here('../web/dist/checkgate.js'));
    const fs = await import('node:fs');
    await wasm.default({ module_or_path: fs.readFileSync(here('../web/dist/checkgate_bg.wasm')) });
    const { CheckgateWeb } = await import(here('../web/index.js'));
    return { name: 'web', engine: 'WASM', ...jsClient(CheckgateWeb, { storage: null }) };
}

export async function reactNativeAdapter() {
    quiet();
    const { register } = await import('node:module');
    register(here('./rn-loader.mjs'), import.meta.url);
    const { ReactNativeXMLHttpRequest } = await import(here('../react-native/test/rn-xhr.mjs'));
    globalThis.XMLHttpRequest = ReactNativeXMLHttpRequest;
    // react-native-sse is CommonJS transpiled from ESM (`__esModule`); Metro's
    // interop unwraps `default`, plain Node does not.
    const sse = requireFrom('../react-native')('react-native-sse');
    globalThis.EventSource = sse.__esModule ? sse.default : sse;
    const makeBridge = await wasmBridge();
    globalThis.__CheckgateInternal = makeBridge();
    const { CheckgateNativeClient } = await import(here('../react-native/index.js'));
    const base = jsClient(CheckgateNativeClient);
    return {
        name: 'react-native', engine: 'WASM behind the JSI bridge interface', ...base,
        create: (url, key, t) => {
            globalThis.__CheckgateInternal = makeBridge();
            return base.create(url, key, t);
        },
    };
}

/** The Dart client, one adapter process per client (tool/conformance_adapter.dart). */
export function flutterAdapter() {
    const dartDir = here('../flutter/dart/');
    const start = () => {
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
    };
    return {
        name: 'flutter',
        engine: process.env.CHECKGATE_FLUTTER_LIB ? 'Rust via FFI' : 'none (CHECKGATE_FLUTTER_LIB unset)',
        async create(url, key, timing) {
            const a = start();
            await a.call('create', { url, key, timing });
            return a;
        },
        async connect(a) {
            const r = await a.call('connect');
            if (!r.ok) {
                const err = new Error(r.error);
                if (/CheckgateAuthException/.test(r.error)) err.name = 'CheckgateAuthError';
                throw err;
            }
        },
        async flags(a) { return new Map(Object.entries((await a.call('flags')).flags)); },
        async evaluate(a, key, user, attrs = {}) {
            const v = (await a.call('evaluate', { key, user, attrs })).value;
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
}

export const ADAPTERS = {
    node: nodeAdapter,
    web: webAdapter,
    'react-native': reactNativeAdapter,
    flutter: async () => flutterAdapter(),
};
