import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { normalizeFlags } from '../src/util.js'
import { generate, resolveLang } from '../src/generate.js'
import { parseArgs } from '../src/args.js'

const raw = JSON.parse(readFileSync(fileURLToPath(new URL('./fixtures/flags.json', import.meta.url)), 'utf8'))
const flags = normalizeFlags(raw)
const meta = { source: 'fixture', count: flags.length }

test('fixture normalizes to 6 flags (archived dropped)', () => {
  assert.equal(flags.length, 6)
  assert.ok(!flags.some(f => f.key === 'archived-thing'))
})

test('resolveLang aliases', () => {
  assert.equal(resolveLang('ts'), 'typescript')
  assert.equal(resolveLang('typescript'), 'typescript')
  assert.equal(resolveLang('rs'), 'rust')
  assert.equal(resolveLang('DART'), 'dart')
  assert.equal(resolveLang('go'), null)
})

test('typescript output shape', () => {
  const out = generate('typescript', flags, meta)
  assert.match(out, /export type FlagKey =/)
  assert.match(out, /\| "new-dashboard"/)
  assert.match(out, /"checkout-color": string/)
  assert.match(out, /"max_items": number/)
  assert.match(out, /"theme-config": unknown/)
  assert.match(out, /"new-dashboard": true,/)
  assert.match(out, /"checkout-color": "blue",/)
  assert.match(out, /"theme-config": \{"mode":"dark","accent":"#10b981"\},/)
  // legacy-flag had no default → boolean zero value.
  assert.match(out, /"legacy-flag": false,/)
  assert.match(out, /export function typedFlags\(client: CheckgateLike\)/)
})

test('dart output shape', () => {
  const out = generate('dart', flags, meta)
  assert.match(out, /enum FlagKey \{/)
  assert.match(out, /newDashboard\('new-dashboard'\)/)
  // leading-digit key gets a safe, public (non-underscore) identifier.
  assert.match(out, /f2faEnabled\('2fa-enabled'\)/)
  assert.doesNotMatch(out, /_2faEnabled/) // never library-private
  assert.match(out, /bool newDashboard\(String userKey/)
  assert.match(out, /String checkoutColor\(String userKey/)
  assert.match(out, /int maxItems\(String userKey/)
  assert.match(out, /dynamic themeConfig\(String userKey/)
  assert.match(out, /class TypedFlags \{/)
})

test('rust output shape', () => {
  const out = generate('rust', flags, meta)
  assert.match(out, /pub enum FlagKey \{/)
  assert.match(out, /NewDashboard,/)
  assert.match(out, /_2faEnabled,/) // leading digit guarded
  assert.match(out, /FlagKey::NewDashboard => "new-dashboard",/)
  assert.match(out, /pub const NEW_DASHBOARD: bool = true;/)
  assert.match(out, /pub const CHECKOUT_COLOR: &str = r#"blue"#;/)
  assert.match(out, /pub const MAX_ITEMS: i64 = 25;/)
  // The JSON default contains the sequence `"#`, so the raw string escalates to `##`.
  assert.match(out, /pub const THEME_CONFIG: &str = r##"\{"mode":"dark","accent":"#10b981"\}"##;/)
})

test('empty flag set generates valid stubs', () => {
  assert.match(generate('typescript', [], meta), /export type FlagKey = never/)
  assert.match(generate('rust', [], meta), /pub enum FlagKey \{\}/)
  assert.match(generate('dart', [], meta), /enum FlagKey \{ none \}/)
})

test('arg parsing', () => {
  assert.deepEqual(
    parseArgs(['typegen', '--lang', 'ts', '-o', 'flags.ts', '--input=./f.json']),
    { command: 'typegen', options: { lang: 'ts', out: 'flags.ts', input: './f.json' }, positional: [] },
  )
  assert.deepEqual(
    parseArgs(['typegen', '--help']),
    { command: 'typegen', options: { help: true }, positional: [] },
  )
})

test('rust: a key of "self" does not become the keyword `Self`', () => {
  const out = generate('rust', normalizeFlags([{ key: 'self', flag_type: 'boolean' }]), meta)
  assert.match(out, /^    SelfFlag,$/m)
  assert.match(out, /FlagKey::SelfFlag => "self",/)
  assert.match(out, /#\[allow\(non_camel_case_types\)\]/)
})
