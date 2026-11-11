// Seeds a fresh server for the benchmarks and writes the credentials the
// harnesses need to a session file.
//
//   node seed.mjs --url http://server:3000 --setup-token <token> --out /work/session.json
//
// Uses only the public setup and flag APIs: the setup wizard (which creates
// the first project, environment and SDK key), then one POST per flag.

import { writeFileSync } from 'node:fs';
import { serverFlags, SERVER_FLAG_COUNT } from './fixtures.mjs';
import { parseArgs } from './util.mjs';

const args = parseArgs({ url: 'http://server:3000', 'setup-token': '', out: '/work/session.json' });
if (!args['setup-token']) throw new Error('--setup-token is required');

async function waitReady(url) {
  for (let i = 0; i < 240; i++) {
    try {
      const r = await fetch(`${url}/health/ready`);
      if (r.ok) return;
    } catch { /* not up yet */ }
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error('server did not become ready');
}

async function must(res, what) {
  if (!res.ok) throw new Error(`${what}: HTTP ${res.status} ${await res.text()}`);
  return res;
}

await waitReady(args.url);
const setupHeaders = { 'X-Checkgate-Setup-Token': args['setup-token'] };
const key = await (await must(await fetch(`${args.url}/api/setup/key`, { headers: setupHeaders }), 'setup key')).json();

const complete = await must(await fetch(`${args.url}/api/setup/complete`, {
  method: 'POST',
  headers: { ...setupHeaders, 'Content-Type': 'application/json', 'X-Checkgate-Request': 'true' },
  body: JSON.stringify({
    workspace_name: 'Bench', project_name: 'Bench', name: 'Admin',
    email: 'admin@bench.test', password: 'bench-password-1',
  }),
}), 'setup complete');
// The session cookie(s), without attributes, as a Cookie header value.
const cookie = complete.headers.getSetCookie().map((c) => c.split(';')[0]).join('; ');
if (!cookie) throw new Error('setup/complete set no session cookie');

const env = key.environment_id;
const admin = { 'Content-Type': 'application/json', 'X-Checkgate-Request': 'true', Cookie: cookie };
for (const flag of serverFlags()) {
  await must(await fetch(`${args.url}/api/environments/${env}/flags`, {
    method: 'POST', headers: admin, body: JSON.stringify(flag),
  }), `create ${flag.key}`);
}

const session = { url: args.url, sdk_key: key.key, environment_id: env, cookie, flags: SERVER_FLAG_COUNT };
writeFileSync(args.out, JSON.stringify(session, null, 2));
console.error(`seeded ${SERVER_FLAG_COUNT} flags into environment ${env}`);
