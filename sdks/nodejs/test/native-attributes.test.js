// Attribute handling of the real native core (not the mocked one used by
// client.test.js). Needs the addon built (`npm run build`); skipped otherwise.
//
// The binding converts only the attributes a flag's rules (and its
// prerequisites' rules) reference. These tests pin what that must preserve and
// what it deliberately changes.

const { test } = require('node:test')
const assert = require('node:assert')

let CheckgateCore
try {
  ({ CheckgateCore } = require('../native-binding.js'))
} catch {
  CheckgateCore = null
}
const opts = { skip: CheckgateCore ? false : 'native addon not built' }

function core(flags) {
  const c = new CheckgateCore()
  for (const f of flags) {
    c.upsertFlagV2(JSON.stringify({ rollout_percentage: null, description: null, ...f }))
  }
  return c
}

const proOnly = {
  key: 'pro-only',
  is_enabled: true,
  rollout_percentage: 0,
  rules: [{ attribute: 'plan', operator: 'equals', values: ['pro'] }],
}

test('referenced attributes still decide the result', opts, () => {
  const c = core([proOnly])
  assert.strictEqual(c.isEnabled('pro-only', 'u', { plan: 'pro' }), true)
  assert.strictEqual(c.isEnabled('pro-only', 'u', { plan: 'free' }), false)
  assert.strictEqual(c.isEnabled('pro-only', 'u', {}), false)
})

test('attributes read by a prerequisite are converted too', opts, () => {
  const c = core([
    proOnly,
    { key: 'dependent', is_enabled: true, prerequisites: [{ flag_key: 'pro-only' }] },
  ])
  assert.strictEqual(c.isEnabled('dependent', 'u', { plan: 'pro' }), true)
  assert.strictEqual(c.isEnabled('dependent', 'u', { plan: 'free' }), false)
})

test('a referenced attribute that is not a string still throws', opts, () => {
  const c = core([proOnly])
  // Same error, same message as v0.1.30's full conversion.
  assert.throws(() => c.isEnabled('pro-only', 'u', { plan: 7 }), /into rust type `String`/)
})

test('an unreferenced non-string attribute no longer throws', opts, () => {
  const c = core([proOnly, { key: 'no-rules', is_enabled: true }])
  assert.strictEqual(c.isEnabled('pro-only', 'u', { plan: 'pro', age: 30 }), true)
  assert.strictEqual(c.isEnabled('no-rules', 'u', { age: 30, beta: true }), true)
})

test('inherited properties are not attributes', opts, () => {
  const c = core([{
    key: 'proto',
    is_enabled: true,
    rollout_percentage: 0,
    rules: [{ attribute: 'toString', operator: 'not_equals', values: ['x'] }],
  }])
  // `toString` exists on every object's prototype; it is not a user attribute,
  // so it is missing — and a missing attribute satisfies not_equals.
  assert.strictEqual(c.isEnabled('proto', 'u', {}), true)
})

test('undefined values and a missing attribute object mean "absent"', opts, () => {
  const c = core([proOnly])
  assert.strictEqual(c.isEnabled('pro-only', 'u', { plan: undefined }), false)
  assert.strictEqual(c.isEnabled('pro-only', 'u', undefined), false)
  assert.strictEqual(c.isEnabled('pro-only', 'u', null), false)
  assert.strictEqual(c.getValue('absent', 'u', {}), 'null')
})
