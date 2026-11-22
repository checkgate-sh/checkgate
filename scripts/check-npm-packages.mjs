#!/usr/bin/env node
// Release check for the npm packages added in 0.2.0, without publishing:
//
//   node scripts/check-npm-packages.mjs [--version X] [--out DIR] [--pack-only]
//
//   @checkgate/openfeature-provider   sdks/openfeature-provider
//   @checkgate/node-integrations      sdks/node-integrations
//   @checkgate/mcp-server             integrations/mcp-server
//
// 1. Pack each one (`npm pack --ignore-scripts`, a real tarball) and check the
//    tarball: every path the manifest points at (main, types, every exports
//    target, bin) is in it, no development files are (tests, benches,
//    examples, lockfiles, tsconfig, env files), bin files start with a
//    shebang, and the three versions agree.
// 2. Unless --pack-only: install the tarballs into an empty consumer project
//    with `@checkgate/node` packed the same way (its native addon must be
//    built: `npm run build` in sdks/nodejs) at the release version — what a
//    user gets once the release is out — and their peer dependencies from
//    the registry. Then, from the consumer: require and import every entry
//    point, type-check the declarations through `exports` (TypeScript,
//    NodeNext), and run each package once against a local stub of the
//    Checkgate stream: the provider and the Express middleware evaluate a
//    flag, and `checkgate-mcp` answers `tools/list` over stdio.
//
// --version is the release version (default: the new packages' version);
// @checkgate/node is packed with it, as scripts/bump-version.sh would set it.
// Needs network access to the npm registry for step 2. Writes nothing to the
// repository; tarballs and the consumer go to --out (default a temp dir).

import { execFileSync, spawn } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const argv = process.argv.slice(2);
const flag = (name) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : undefined;
};
const packOnly = argv.includes('--pack-only');
const out = path.resolve(flag('--out') ?? mkdtempSync(path.join(os.tmpdir(), 'checkgate-npm-')));
mkdirSync(out, { recursive: true });

const PACKAGES = [
  { dir: 'sdks/openfeature-provider', name: '@checkgate/openfeature-provider' },
  { dir: 'sdks/node-integrations', name: '@checkgate/node-integrations' },
  { dir: 'integrations/mcp-server', name: '@checkgate/mcp-server' },
];
// Paths a published package must not carry.
const FORBIDDEN = [
  /(^|\/)test(s)?\//, /(^|\/)bench\//, /(^|\/)examples?\//, /\.test\.[cm]?[jt]s$/,
  /(^|\/)package-lock\.json$/, /(^|\/)tsconfig[^/]*\.json$/, /(^|\/)\.env/, /(^|\/)node_modules\//,
  /\.tgz$/, /(^|\/)\.github\//,
];

const failures = [];
const report = { out, packages: {}, consumer: null };
const fail = (msg) => {
  failures.push(msg);
  console.error(`  FAIL ${msg}`);
};
const ok = (msg) => console.log(`  ok   ${msg}`);
const npm = (args, cwd, opts = {}) =>
  execFileSync(process.platform === 'win32' ? 'npm.cmd' : 'npm', args, {
    cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', opts.quiet ? 'ignore' : 'inherit'],
    env: { ...process.env, npm_config_audit: 'false', npm_config_fund: 'false', npm_config_update_notifier: 'false' },
  });

function targets(exp) {
  if (typeof exp === 'string') return [exp];
  if (exp && typeof exp === 'object') return Object.values(exp).flatMap(targets);
  return [];
}

function pack(dir, name) {
  const manifest = JSON.parse(readFileSync(path.join(dir, 'package.json'), 'utf8'));
  const [info] = JSON.parse(npm(['pack', '--ignore-scripts', '--json', '--pack-destination', out], dir));
  const files = new Set(info.files.map((f) => f.path));
  const referenced = new Set();
  for (const k of ['main', 'types', 'typings', 'module']) if (manifest[k]) referenced.add(manifest[k]);
  for (const t of targets(manifest.exports)) referenced.add(t);
  const bins = typeof manifest.bin === 'string' ? [manifest.bin] : Object.values(manifest.bin ?? {});
  for (const b of bins) referenced.add(b);
  const norm = (p) => p.replace(/^\.\//, '');
  const missing = [...new Set([...referenced].map(norm))].filter((p) => p !== 'package.json' && !files.has(p));
  const forbidden = [...files].filter((p) => FORBIDDEN.some((re) => re.test(p)));
  for (const b of bins) {
    const src = readFileSync(path.join(dir, b), 'utf8');
    if (!src.startsWith('#!/usr/bin/env node')) fail(`${name}: bin ${b} has no node shebang`);
  }
  const entry = {
    version: manifest.version, tarball: info.filename, size: info.size, unpacked: info.unpackedSize,
    integrity: info.integrity, files: [...files].sort(), referenced: [...new Set([...referenced].map(norm))].sort(),
    peerDependencies: manifest.peerDependencies ?? {}, dependencies: manifest.dependencies ?? {},
    engines: manifest.engines ?? {},
  };
  if (missing.length) fail(`${name}: manifest points at files the tarball lacks: ${missing.join(', ')}`);
  else ok(`${name}@${manifest.version}: ${files.size} files, every manifest path present (${entry.referenced.join(', ')})`);
  if (forbidden.length) fail(`${name}: development files in the tarball: ${forbidden.join(', ')}`);
  else ok(`${name}: no development files`);
  for (const must of ['package.json', 'README.md', 'LICENSE']) {
    if (!files.has(must)) fail(`${name}: tarball has no ${must}`);
  }
  return entry;
}

// Tiny semver range check for the forms the manifests use (>=, <, ^, x.y.z, ||).
function satisfies(version, range) {
  const v = version.split('.').map(Number);
  const cmp = (a, b) => a[0] - b[0] || a[1] - b[1] || a[2] - b[2];
  const parse = (s) => s.split('.').map(Number).concat([0, 0, 0]).slice(0, 3);
  return range.split('||').some((set) =>
    set.trim().split(/\s+/).every((c) => {
      const m = c.match(/^(>=|<=|>|<|\^|~|=)?(.+)$/);
      const t = parse(m[2]);
      switch (m[1]) {
        case '>=': return cmp(v, t) >= 0;
        case '>': return cmp(v, t) > 0;
        case '<=': return cmp(v, t) <= 0;
        case '<': return cmp(v, t) < 0;
        case '^': return cmp(v, t) >= 0 && (t[0] > 0 ? v[0] === t[0] : v[1] === t[1]);
        case '~': return cmp(v, t) >= 0 && v[0] === t[0] && v[1] === t[1];
        default: return cmp(v, t) === 0;
      }
    }));
}

console.log(`Packing into ${out}`);
for (const p of PACKAGES) report.packages[p.name] = pack(path.join(root, p.dir), p.name);
const versions = new Set(Object.values(report.packages).map((p) => p.version));
const version = flag('--version') ?? [...versions][0];
if (versions.size !== 1) fail(`versions differ: ${[...versions].join(', ')}`);
else ok(`one version for the new packages: ${version}`);
for (const [name, p] of Object.entries(report.packages)) {
  const range = p.peerDependencies['@checkgate/node'];
  if (range && !satisfies(version, range)) fail(`${name}: peer @checkgate/node "${range}" excludes the release version ${version}`);
  else if (range) ok(`${name}: peer @checkgate/node "${range}" accepts ${version}`);
}

if (!packOnly) {
  console.log('Clean consumer install');
  // @checkgate/node at the release version, packed from a copy so the
  // repository is untouched.
  const nodeSrc = path.join(root, 'sdks/nodejs');
  const addon = readdirNode(nodeSrc);
  if (!addon.length) {
    fail('sdks/nodejs has no built native addon (*.node): run `npm run build` there first');
  } else {
    const copy = path.join(out, 'node-sdk');
    mkdirSync(copy, { recursive: true });
    const nodeManifest = JSON.parse(readFileSync(path.join(nodeSrc, 'package.json'), 'utf8'));
    for (const f of [...new Set([...(nodeManifest.files ?? []).filter((f) => !f.includes('*')), ...addon, 'package.json', 'README.md', 'LICENSE'])]) {
      if (existsSync(path.join(nodeSrc, f))) cpSync(path.join(nodeSrc, f), path.join(copy, f));
    }
    const m = JSON.parse(readFileSync(path.join(copy, 'package.json'), 'utf8'));
    m.version = version;
    delete m.scripts;
    writeFileSync(path.join(copy, 'package.json'), JSON.stringify(m, null, 2));
    const [nodeInfo] = JSON.parse(npm(['pack', '--ignore-scripts', '--json', '--pack-destination', out], copy));
    ok(`@checkgate/node@${version} packed from sdks/nodejs with ${addon.join(', ')}`);

    const consumer = path.join(out, 'consumer');
    mkdirSync(consumer, { recursive: true });
    writeFileSync(path.join(consumer, 'package.json'), JSON.stringify({ name: 'consumer', private: true, version: '0.0.0' }, null, 2));
    const tarballs = [nodeInfo.filename, ...Object.values(report.packages).map((p) => p.tarball)].map((t) => path.join(out, t));
    const devDeps = (dir) => JSON.parse(readFileSync(path.join(root, dir, 'package.json'), 'utf8')).devDependencies ?? {};
    const pin = (deps, names) => names.map((n) => `${n}@${deps[n]}`);
    const thirdParty = [
      ...pin(devDeps('sdks/openfeature-provider'), ['@openfeature/server-sdk', 'typescript', '@types/node']),
      ...pin(devDeps('sdks/node-integrations'), ['express', 'fastify', '@nestjs/common', '@nestjs/core', 'reflect-metadata', 'rxjs', '@types/express']),
    ];
    try {
      npm(['install', '--no-audit', '--no-fund', '--loglevel=error', ...tarballs, ...thirdParty], consumer);
      ok(`installed ${tarballs.length} tarballs and ${thirdParty.length} peer/tooling packages`);
      writeFileSync(path.join(consumer, 'smoke.cjs'), smokeSource());
      writeFileSync(path.join(consumer, 'smoke-types.mts'), typesSource());
      writeFileSync(path.join(consumer, 'tsconfig.json'), JSON.stringify({
        compilerOptions: { module: 'nodenext', moduleResolution: 'nodenext', target: 'es2022', strict: true, noEmit: true, skipLibCheck: false, types: ['node'] },
        files: ['smoke-types.mts'],
      }, null, 2));
      try {
        execFileSync(process.execPath, [path.join(consumer, 'node_modules/typescript/bin/tsc'), '-p', 'tsconfig.json'], { cwd: consumer, encoding: 'utf8', stdio: 'pipe' });
        ok('declarations type-check through exports (TypeScript, NodeNext, strict)');
      } catch (e) {
        fail(`type check: ${e.stdout || e.message}`);
      }
      const result = await runSmoke(consumer);
      report.consumer = result;
      for (const [k, v] of Object.entries(result)) (v === true ? ok : fail)(`smoke ${k}${v === true ? '' : `: ${v}`}`);
    } catch (e) {
      fail(`consumer install: ${e.message}`);
    }
  }
}

writeFileSync(path.join(out, 'report.json'), JSON.stringify({ ...report, failures }, null, 2));
console.log(failures.length ? `${failures.length} failure(s); report ${path.join(out, 'report.json')}` : `all checks passed; report ${path.join(out, 'report.json')}`);
process.exit(failures.length ? 1 : 0);

function readdirNode(dir) {
  return execFileSync('ls', [dir], { encoding: 'utf8' }).split('\n').filter((f) => f.endsWith('.node'));
}

function runSmoke(consumer) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, ['smoke.cjs'], { cwd: consumer, stdio: ['ignore', 'pipe', 'inherit'] });
    let text = '';
    child.stdout.on('data', (d) => { text += d; });
    const timer = setTimeout(() => child.kill('SIGKILL'), 60_000);
    child.on('exit', (code) => {
      clearTimeout(timer);
      try {
        resolve(JSON.parse(text.trim().split('\n').pop()));
      } catch {
        resolve({ run: `smoke exited ${code} without a result` });
      }
    });
  });
}

// The consumer's behaviour check (CommonJS, run from the consumer directory).
function smokeSource() {
  return String.raw`'use strict'
const http = require('node:http')
const assert = require('node:assert/strict')
const path = require('node:path')
const { spawn } = require('node:child_process')
const results = {}
const out = (s) => process.stdout.write(s + '\n')
console.log = () => {} // the SDK logs each (re)connect
const flag = { key: 'smoke', is_enabled: true, rollout_percentage: null, description: null, rules: [] }
const stub = http.createServer((req, res) => {
  if (req.url.startsWith('/stream')) {
    res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache' })
    res.write('event: connected\ndata: ' + JSON.stringify({ environment_id: 'smoke-env' }) + '\n\n')
    res.write('event: update\ndata: ' + JSON.stringify({ type: 'UPSERT', env_id: 'smoke-env', revision: 1, flag }) + '\n\n')
    res.write('event: ready\ndata: 1\n\n')
    return
  }
  if (req.url.startsWith('/flags/snapshot')) {
    res.writeHead(200, { 'Content-Type': 'application/json' })
    res.end(JSON.stringify([flag]))
    return
  }
  res.writeHead(404, { 'Content-Type': 'application/json' }).end('{"error":"not found"}')
})
async function step(name, fn) {
  try { await fn(); results[name] = true } catch (e) { results[name] = String((e && e.stack) || e).split('\n').slice(0, 3).join(' | ') }
}
;(async () => {
  await new Promise((r) => stub.listen(0, '127.0.0.1', r))
  const serverUrl = 'http://127.0.0.1:' + stub.address().port
  let client
  await step('node_sdk_evaluates', async () => {
    const { CheckgateClient } = require('@checkgate/node')
    client = new CheckgateClient({ serverUrl, sdkKey: 'smoke', reportImpressions: false })
    await client.connect()
    assert.equal(client.isEnabled('smoke', 'user-1', {}), true)
    assert.equal(client.isEnabled('absent', 'user-1', {}), false)
  })
  await step('openfeature_provider_evaluates', async () => {
    const { OpenFeature } = require('@openfeature/server-sdk')
    const { CheckgateProvider } = require('@checkgate/openfeature-provider')
    await OpenFeature.setProviderAndWait(new CheckgateProvider({ client }))
    const of = OpenFeature.getClient()
    assert.equal(await of.getBooleanValue('smoke', false, { targetingKey: 'user-1' }), true)
    const missing = await of.getBooleanDetails('absent', true, { targetingKey: 'user-1' })
    assert.equal(missing.value, true, 'a missing flag returns the caller default')
  })
  await step('express_middleware_evaluates', async () => {
    const express = require('express')
    const { checkgateMiddleware } = require('@checkgate/node-integrations/express')
    const app = express()
    app.use(checkgateMiddleware({ client, context: () => ({ key: 'user-1' }) }))
    app.get('/', (req, res) => res.json({ v: req.checkgate.isEnabled('smoke') }))
    const srv = await new Promise((r) => { const s = app.listen(0, '127.0.0.1', () => r(s)) })
    const body = await (await fetch('http://127.0.0.1:' + srv.address().port + '/')).json()
    srv.close()
    assert.deepEqual(body, { v: true })
  })
  await step('entry_points_require', async () => {
    require('reflect-metadata')
    for (const m of ['@checkgate/openfeature-provider', '@checkgate/node-integrations', '@checkgate/node-integrations/express',
      '@checkgate/node-integrations/fastify', '@checkgate/node-integrations/nestjs']) require(m)
  })
  await step('entry_points_import', async () => {
    for (const m of ['@checkgate/openfeature-provider', '@checkgate/node-integrations', '@checkgate/node-integrations/express',
      '@checkgate/node-integrations/fastify', '@checkgate/node-integrations/nestjs', '@checkgate/mcp-server']) await import(m)
  })
  await step('mcp_server_lists_tools', async () => {
    const bin = path.join(__dirname, 'node_modules', '.bin', process.platform === 'win32' ? 'checkgate-mcp.cmd' : 'checkgate-mcp')
    const child = spawn(bin, [], { env: { ...process.env, CHECKGATE_URL: serverUrl, CHECKGATE_TOKEN: 'cg_pat_smoke' }, stdio: ['pipe', 'pipe', 'pipe'] })
    const responses = new Map()
    let buf = ''
    child.stdout.on('data', (d) => {
      buf += d
      let i
      while ((i = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, i); buf = buf.slice(i + 1)
        try { const m = JSON.parse(line); if (m.id !== undefined) responses.set(m.id, m) } catch {}
      }
    })
    const wait = (id) => new Promise((resolve, reject) => {
      const t0 = Date.now()
      const iv = setInterval(() => {
        if (responses.has(id)) { clearInterval(iv); resolve(responses.get(id)) }
        else if (Date.now() - t0 > 15000) { clearInterval(iv); reject(new Error('no response to request ' + id)) }
      }, 20)
    })
    const send = (m) => child.stdin.write(JSON.stringify(m) + '\n')
    try {
      send({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'smoke', version: '0' } } })
      const init = await wait(1)
      assert.ok(init.result && init.result.serverInfo, JSON.stringify(init))
      send({ jsonrpc: '2.0', method: 'notifications/initialized' })
      send({ jsonrpc: '2.0', id: 2, method: 'tools/list' })
      const names = (await wait(2)).result.tools.map((t) => t.name)
      assert.ok(names.length > 0, 'no tools')
      assert.ok(!names.some((n) => /propose|create|approve|reject|cancel/.test(n)), 'read-only by default, got ' + names)
    } finally {
      child.kill()
    }
  })
  if (client) client.disconnect()
  stub.close()
  out(JSON.stringify(results))
  process.exit(0)
})()
`;
}

// Declarations, resolved through each package's exports (NodeNext, strict).
function typesSource() {
  return `import { CheckgateClient } from '@checkgate/node';
import { CheckgateProvider, toAttributes } from '@checkgate/openfeature-provider';
import { checkgateMiddleware, listenWithCheckgate } from '@checkgate/node-integrations/express';
import checkgatePlugin from '@checkgate/node-integrations/fastify';
import { CheckgateModule, RequireFlag } from '@checkgate/node-integrations/nestjs';
import { startCheckgate } from '@checkgate/node-integrations';
const client = new CheckgateClient({ serverUrl: 'http://127.0.0.1:1', sdkKey: 'k' });
const provider: CheckgateProvider = new CheckgateProvider({ client });
const attrs: Record<string, string> = toAttributes({ targetingKey: 'u', plan: 'pro' });
const mw = checkgateMiddleware({ client, context: () => ({ key: 'u' }) });
export const used = [provider, attrs, mw, listenWithCheckgate, checkgatePlugin, CheckgateModule, RequireFlag, startCheckgate];
`;
}
