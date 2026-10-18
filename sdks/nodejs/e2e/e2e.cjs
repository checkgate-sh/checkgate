'use strict'

// Native end-to-end test: drives the REAL @checkgate/node SDK (native NAPI core
// + a real EventSource SSE connection) against a running Checkgate server.
//
// Proves the full loop an application sees: connect over SSE, receive the flag
// bootstrap, evaluate locally, and have live REST changes propagate to local
// evaluation in real time.
//
// Prerequisites (provided by the runner — see the shell/CI orchestration):
//   - The native addon is built (`npm run build:debug` → native-binding.js).
//   - A server is running at CHECKGATE_URL with a FRESH (un-set-up) database.
//
// Env: CHECKGATE_URL (default http://127.0.0.1:3000)

const assert = require('node:assert')
const { CheckgateClient } = require('../index.js')

const BASE = process.env.CHECKGATE_URL || 'http://127.0.0.1:3000'

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

async function api(method, path, { token, cookie, body } = {}) {
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers: {
      'Content-Type': 'application/json',
      'X-Checkgate-Request': 'true',
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...(cookie ? { Cookie: cookie } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  })
  return res
}

// Poll a predicate until true or timeout — used to wait for SSE propagation.
async function waitFor(label, predicate, timeoutMs = 8000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (predicate()) return
    await sleep(100)
  }
  throw new Error(`timed out waiting for: ${label}`)
}

async function main() {
  // 1. First-run setup: grab the seeded SDK key + environment, create the admin.
  const keyRes = await api('GET', '/api/setup/key')
  assert.equal(keyRes.status, 200, 'GET /api/setup/key')
  const { key: sdkKey, environment_id: env } = await keyRes.json()

  const setupRes = await api('POST', '/api/setup/complete', {
    body: { workspace_name: 'E2E', project_name: 'P', name: 'A', email: 'a@e.test', password: 'supersecret1' },
  })
  assert.equal(setupRes.status, 200, 'POST /api/setup/complete')

  // SDK keys can't manage flags, so the "dashboard" side of this test uses an
  // admin personal access token, minted with the setup session.
  const cookie = setupRes.headers.getSetCookie().map((c) => c.split(';')[0]).join('; ')
  const tokenRes = await api('POST', '/api/tokens', { cookie, body: { name: 'e2e', scope: 'read_write' } })
  assert.equal(tokenRes.status, 200, 'POST /api/tokens')
  const { token: adminToken } = await tokenRes.json()
  assert.equal(
    (await api('POST', `/api/environments/${env}/flags`, { token: sdkKey, body: { key: 'sdk-key-write', is_enabled: true, rules: [] } })).status,
    403,
    'an SDK key cannot create flags',
  )

  const flags = `/api/environments/${env}/flags`

  // 2. A flag that exists before the SDK connects — must arrive in the bootstrap.
  assert.equal(
    (await api('POST', flags, { token: adminToken, body: { key: 'bootstrap-flag', is_enabled: true, rollout_percentage: 100, flag_type: 'boolean', rules: [] } })).status,
    200,
    'create bootstrap-flag',
  )

  // 3. Connect the REAL SDK over SSE and evaluate locally.
  const client = new CheckgateClient({ serverUrl: BASE, sdkKey })
  try {
    await client.connect()
    assert.equal(client.isEnabled('bootstrap-flag', 'user-1'), true, 'bootstrap flag evaluates true after SSE replay')

    // 4. Create a NEW flag over REST — it must propagate to the SDK via SSE.
    await api('POST', flags, { token: adminToken, body: { key: 'pushed-flag', is_enabled: true, rollout_percentage: 100, flag_type: 'boolean', rules: [] } })
    await waitFor('pushed-flag → enabled locally', () => client.isEnabled('pushed-flag', 'user-1') === true)

    // 5. Disable it over REST — the change propagates and local eval flips.
    await api('PATCH', `${flags}/pushed-flag`, { token: adminToken, body: { is_enabled: false } })
    await waitFor('pushed-flag → disabled locally', () => client.isEnabled('pushed-flag', 'user-1') === false)

    // 6. A variant flag + getValue round-trip.
    await api('POST', flags, { token: adminToken, body: { key: 'color', is_enabled: true, rollout_percentage: 100, flag_type: 'string', default_value: 'blue', rules: [] } })
    await waitFor('color flag present locally', () => client.getVariant('color', 'user-1') !== null)
    assert.equal(client.getValue('color', 'user-1', {}, 'fallback'), 'blue', 'string flag value evaluates locally')

    // 7. Report a goal event (best-effort; just exercise the real code path).
    client.track('checkout_complete', 'user-1', { value: 9.99 })

    console.log('native-e2e: PASSED (SSE bootstrap + live UPSERT/PATCH + local eval)')
  } finally {
    client.disconnect()
  }
}

main().then(
  () => process.exit(0),
  (err) => {
    console.error('native-e2e: FAILED —', err && err.message ? err.message : err)
    process.exit(1)
  },
)
