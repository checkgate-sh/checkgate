// Development-only seeding for the quickstart: completes first-run setup with
// the one-time setup token (exactly what the wizard does), creates the sample
// flag `new-checkout`, and writes the SDK key to quickstart/.env.
// The admin password is random, printed once, and never stored.
import { randomBytes } from 'node:crypto';
import { appendFileSync, readFileSync } from 'node:fs';

const base = process.env.CHECKGATE_URL || 'http://localhost:3000';
const token = process.env.SETUP_TOKEN;
if (!token) throw new Error('SETUP_TOKEN is not set (run quickstart/start.sh --seed).');

let cookie = '';
async function call(method, path, body, headers = {}) {
    const res = await fetch(`${base}${path}`, {
        method,
        headers: { 'Content-Type': 'application/json', 'X-Checkgate-Request': 'true', ...(cookie ? { cookie } : {}), ...headers },
        body: body ? JSON.stringify(body) : undefined,
    });
    const set = res.headers.getSetCookie?.() ?? [];
    if (set.length) cookie = set.map((c) => c.split(';')[0]).join('; ');
    const text = await res.text();
    if (!res.ok) throw new Error(`${method} ${path}: HTTP ${res.status} ${text.slice(0, 200)}`);
    return text ? JSON.parse(text) : null;
}

const key = await call('GET', '/api/setup/key', null, { 'X-Checkgate-Setup-Token': token });
const email = 'admin@example.com';
// QUICKSTART_ADMIN_PASSWORD exists for the automated smoke test only.
const password = process.env.QUICKSTART_ADMIN_PASSWORD || randomBytes(12).toString('base64url');
await call('POST', '/api/setup/complete', {
    workspace_name: 'Quickstart', project_name: 'My App', name: 'Admin', email, password,
}, { 'X-Checkgate-Setup-Token': token });
await call('POST', `/api/environments/${key.environment_id}/flags`, {
    key: 'new-checkout', is_enabled: true, rollout_percentage: 0,
    description: 'Quickstart sample: on for the pro plan',
    rules: [{ attribute: 'plan', operator: 'equals', values: ['pro'] }],
});

const envFile = new URL('./.env', import.meta.url);
if (!readFileSync(envFile, 'utf8').includes('CHECKGATE_SDK_KEY=')) {
    appendFileSync(envFile, `CHECKGATE_SDK_KEY=${key.key}\nCHECKGATE_ENVIRONMENT_ID=${key.environment_id}\n`);
}
console.log(`
Setup complete (development only).
  Dashboard:   ${base}   ${email} / ${password}   (shown once)
  Environment: ${key.environment_name}
  SDK key:     written to quickstart/.env as CHECKGATE_SDK_KEY
  Sample flag: new-checkout (on for plan=pro)

Next:
  (cd quickstart/node && npm install && node --env-file=../.env index.mjs)
  (cd quickstart/python && python3 -m venv .venv && .venv/bin/pip install checkgate && set -a && . ../.env && set +a && .venv/bin/python main.py)
`);
