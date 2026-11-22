// The shared evaluation vectors (sdks/conformance/vectors/evaluation.json),
// through the public client API with the real native engine. Needs the addon
// built (`npm run build`); skipped otherwise.
//
// The flags reach the client the way an offline start does: hydrated from a
// storage adapter, with no server reachable. Every SDK checks the same file;
// the Rust core is the oracle that produced `expect`.

const { test } = require('node:test')
const assert = require('node:assert')
const fs = require('node:fs')
const path = require('node:path')

let native = null
try {
  native = require('../native-binding.js')
} catch {
  native = null
}
const opts = { skip: native ? false : 'native addon not built' }

const vectors = JSON.parse(
  fs.readFileSync(path.join(__dirname, '../../conformance/vectors/evaluation.json'), 'utf8'),
)

/** A client holding exactly the vector flags, hydrated offline. */
async function hydratedClient() {
  const { CheckgateClient } = require('../index.js')
  const snapshot = Object.fromEntries(vectors.flags.map((f) => [f.key, f]))
  const client = new CheckgateClient({
    // Nothing listens here: the client stays offline on the hydrated flags.
    serverUrl: 'http://127.0.0.1:9',
    reportImpressions: false,
    reconnectDelayMs: 60_000,
    storage: { getItem: () => JSON.stringify(snapshot), setItem: () => {} },
  })
  client.connect().catch(() => {})
  // Hydration is one awaited getItem; give it a turn of the event loop.
  for (let i = 0; i < 50 && !client._hydrated; i++) await new Promise((r) => setTimeout(r, 10))
  assert.ok(client._hydrated, 'client hydrated from storage')
  return client
}

/** `getDetails` in the vector file's snake_case shape. */
function asVector(details) {
  if (details === null) return null
  const out = { enabled: details.enabled, value: details.value, reason: details.reason }
  if (details.ruleIndex !== undefined) out.rule_index = details.ruleIndex
  if (details.rolloutBucket !== undefined) out.rollout_bucket = details.rolloutBucket
  if (details.variantIndex !== undefined) out.variant_index = details.variantIndex
  return out
}

test('every vector case evaluates as the core says', opts, async () => {
  const quiet = console.warn
  console.warn = () => {}
  const client = await hydratedClient()
  try {
    assert.ok(vectors.cases.length > 50)
    for (const c of vectors.cases) {
      const details = client.getDetails(c.flag, c.user, c.attributes)
      assert.deepStrictEqual(asVector(details), c.expect, c.name)
      if (c.expect === null) {
        assert.strictEqual(client.isEnabled(c.flag, c.user, c.attributes), false, c.name)
        assert.strictEqual(client.getVariant(c.flag, c.user, c.attributes), null, c.name)
        assert.strictEqual(client.getValue(c.flag, c.user, c.attributes, 'dflt'), 'dflt', c.name)
        continue
      }
      // The plain APIs agree with the detailed one.
      assert.strictEqual(client.isEnabled(c.flag, c.user, c.attributes), c.expect.enabled, c.name)
      assert.deepStrictEqual(
        client.getVariant(c.flag, c.user, c.attributes),
        { enabled: c.expect.enabled, value: c.expect.value },
        c.name,
      )
      assert.deepStrictEqual(client.getValue(c.flag, c.user, c.attributes), c.expect.value, c.name)
    }
  } finally {
    client.disconnect()
    console.warn = quiet
  }
})

test('getDetails reports prerequisites in camelCase', opts, async () => {
  const quiet = console.warn
  console.warn = () => {}
  const client = await hydratedClient()
  try {
    const d = client.getDetails('needs-chain', 'user-1', {})
    assert.strictEqual(d.reason, 'PREREQUISITE_FAILED')
    assert.deepStrictEqual(d.prerequisites.map((p) => [p.flagKey, p.outcome]), [
      ['static-on', 'satisfied'],
      ['needs-base', 'unsatisfied'],
    ])
    assert.deepStrictEqual(d.prerequisites[1].result, { enabled: false, value: false })
    const missing = client.getDetails('needs-missing', 'user-1', {})
    assert.deepStrictEqual(missing.prerequisites, [{ flagKey: 'ghost', outcome: 'not_found' }])
  } finally {
    client.disconnect()
    console.warn = quiet
  }
})

test('malformed configurations are never loaded', opts, () => {
  const core = new native.CheckgateCore()
  for (const bad of vectors.malformed) {
    core.upsertFlagV2(bad.json)
  }
  for (const key of ['bad-rollout', 'bad-op', 'bad-type', 'neg-weight']) {
    assert.strictEqual(core.explain(key, 'u', {}), 'null', key)
    assert.strictEqual(core.isEnabled(key, 'u', {}), false, key)
  }
})
