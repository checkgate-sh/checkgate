// Automated quickstart check, after `start.sh --seed`: the SDK connects,
// evaluates locally, and sees a dashboard change. Prints timings as JSON.
//   QUICKSTART_ADMIN_PASSWORD=... node smoke.mjs
import { createRequire } from 'node:module';
const require = createRequire(new URL('./node/package.json', import.meta.url));
const { CheckgateClient } = require('@checkgate/node');
for (const level of ['log', 'warn']) console[level] = () => {};

const base = `http://localhost:${process.env.CHECKGATE_PORT || 3000}`;
const t = () => performance.now();
let cookie = '';
async function call(method, path, body) {
    const res = await fetch(`${base}${path}`, {
        method,
        headers: { 'Content-Type': 'application/json', 'X-Checkgate-Request': 'true', ...(cookie ? { cookie } : {}) },
        body: body ? JSON.stringify(body) : undefined,
    });
    const set = res.headers.getSetCookie?.() ?? [];
    if (set.length) cookie = set.map((c) => c.split(';')[0]).join('; ');
    if (!res.ok) throw new Error(`${method} ${path}: HTTP ${res.status}`);
    return res.status === 204 ? null : res.json();
}

const out = {};
let start = t();
const client = new CheckgateClient({ serverUrl: base, sdkKey: process.env.CHECKGATE_SDK_KEY, reportImpressions: false });
await client.connect();
out.sdk_connect_ms = Math.round(t() - start);
const pro = client.isEnabled('new-checkout', 'user-pro', { plan: 'pro' });
const free = client.isEnabled('new-checkout', 'user-free', { plan: 'free' });
if (pro !== true || free !== false) throw new Error(`unexpected first evaluation: pro=${pro} free=${free}`);

await call('POST', '/api/auth/login', { email: 'admin@example.com', password: process.env.QUICKSTART_ADMIN_PASSWORD });
const changed = new Promise((resolve) => client.onChange((key) => key === 'new-checkout' && resolve()));
start = t();
await call('PATCH', `/api/environments/${process.env.CHECKGATE_ENVIRONMENT_ID}/flags/new-checkout`, { is_enabled: false });
await changed;
out.propagation_ms = Math.round(t() - start);
if (client.isEnabled('new-checkout', 'user-pro', { plan: 'pro' }) !== false) throw new Error('change not applied');
client.disconnect();
console.error(JSON.stringify(out));
