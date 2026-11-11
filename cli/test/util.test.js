import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  normalizeFlags, pascalCase, camelCase, upperSnakeCase,
  dedupeIdentifiers, safeDartIdent, zeroValue, tsLiteral, dartLiteral, rustLiteral,
} from '../src/util.js'

test('normalizeFlags drops archived flags and sorts by key', () => {
  const raw = [
    { key: 'b', flag_type: 'string', default_value: 'x' },
    { key: 'a', flag_type: 'boolean', default_value: true },
    { key: 'z', flag_type: 'boolean', archived_at: '2026-01-01T00:00:00Z' },
  ]
  const flags = normalizeFlags(raw)
  assert.deepEqual(flags.map(f => f.key), ['a', 'b'])
})

test('normalizeFlags defaults missing flag_type to boolean and default to undefined', () => {
  const [f] = normalizeFlags([{ key: 'legacy', is_enabled: false }])
  assert.equal(f.type, 'boolean')
  assert.equal(f.default, undefined)
})

test('normalizeFlags de-duplicates by key (last wins)', () => {
  const flags = normalizeFlags([
    { key: 'a', flag_type: 'boolean' },
    { key: 'a', flag_type: 'string', default_value: 'v' },
  ])
  assert.equal(flags.length, 1)
  assert.equal(flags[0].type, 'string')
})

test('normalizeFlags rejects non-arrays', () => {
  assert.throws(() => normalizeFlags({ items: [] }), /array of flags/)
})

test('casing helpers', () => {
  assert.equal(pascalCase('new-dashboard'), 'NewDashboard')
  assert.equal(camelCase('new-dashboard'), 'newDashboard')
  assert.equal(upperSnakeCase('new-dashboard'), 'NEW_DASHBOARD')
  assert.equal(camelCase('max_items'), 'maxItems')
})

test('casing helpers guard against leading digits', () => {
  assert.equal(pascalCase('2fa-enabled'), '_2faEnabled')
  assert.equal(camelCase('2fa-enabled'), '_2faEnabled')
  assert.equal(upperSnakeCase('2fa-enabled'), '_2FA_ENABLED')
})

test('dedupeIdentifiers suffixes collisions', () => {
  assert.deepEqual(dedupeIdentifiers(['aB', 'aB', 'c']), ['aB', 'aB_2', 'c'])
})

test('safeDartIdent avoids reserved words and re-publicizes leading underscores', () => {
  assert.equal(safeDartIdent('class'), 'classFlag')
  assert.equal(safeDartIdent('newDashboard'), 'newDashboard')
  assert.equal(safeDartIdent('_2faEnabled'), 'f2faEnabled')
})

test('zeroValue per type', () => {
  assert.equal(zeroValue('boolean'), false)
  assert.equal(zeroValue('string'), '')
  assert.equal(zeroValue('integer'), 0)
  assert.equal(zeroValue('json'), null)
})

test('literal emitters', () => {
  assert.equal(tsLiteral('blue'), '"blue"')
  assert.equal(dartLiteral('a$b'), "'a\\$b'")
  assert.equal(rustLiteral(true, 'bool'), 'true')
  assert.equal(rustLiteral(25, 'i64'), '25')
  assert.equal(rustLiteral('blue', 'str'), 'r#"blue"#')
  assert.equal(rustLiteral({ a: 1 }, 'json'), 'r#"{"a":1}"#')
})

// --- Regression tests: generated code that did not compile -------------------

test('dartLiteral escapes CR and other control characters (a raw CR broke the Dart string)', () => {
  assert.equal(dartLiteral('a\r\nb'), "'a\\r\\nb'")
  assert.equal(dartLiteral('t\tn\u0000b\u0007d\u007f'), "'t\\tn\\u{0}b\\u{7}d\\u{7f}'")
})

test('rustLiteral uses an escaped literal when the text has a CR (bare CR is illegal in raw strings)', () => {
  assert.equal(rustLiteral('a\r\nb"\\', 'str'), '"a\\r\\nb\\"\\\\"')
  assert.equal(rustLiteral('blue', 'str'), 'r#"blue"#') // unchanged otherwise
})

test('safeDartIdent suffixes names Dart reserves or the generated file shadows', () => {
  for (const n of ['values', 'index', 'hashCode', 'toString', 'runtimeType', 'noSuchMethod', 'dynamic', 'bool', 'int', 'key']) {
    assert.equal(safeDartIdent(n), `${n}Flag`)
  }
  assert.equal(safeDartIdent('name'), 'name')
})

test('normalizeFlags rejects defaults that do not match the flag type, naming each flag', () => {
  assert.throws(
    () => normalizeFlags([
      { key: 'n', flag_type: 'integer', default_value: 1.5 },
      { key: 'big', flag_type: 'integer', default_value: 2 ** 53 },
      { key: 's', flag_type: 'string', default_value: 3 },
      { key: 'ok', flag_type: 'json', default_value: 'anything' },
    ]),
    err => {
      assert.equal(err.code, 'invalid_input')
      assert.deepEqual(err.findings.map(f => f.flag), ['big', 'n', 's'])
      return true
    },
  )
})

test('normalizeFlags treats a null default on a non-json flag as unset', () => {
  const [b, j] = normalizeFlags([
    { key: 'b', flag_type: 'boolean', default_value: null },
    { key: 'j', flag_type: 'json', default_value: null },
  ])
  assert.equal(b.default, undefined)
  assert.equal(j.default, null)
})
