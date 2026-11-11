// Shared helpers: flag normalization, identifier casing, and language literal
// emitters used by the code generators.

import { CliError } from './errors.js'

/** Valid flag types (mirrors the server's `FlagType` enum). */
export const FLAG_TYPES = ['boolean', 'string', 'integer', 'json']

/** Describe what a default of `type` must be, or null when `value` is acceptable. */
function defaultMismatch(type, value) {
  switch (type) {
    case 'boolean': return typeof value === 'boolean' ? null : 'a boolean'
    case 'string': return typeof value === 'string' ? null : 'a string'
    // Beyond 2^53 a JSON number has already lost precision when parsed, so the
    // generated constant would silently differ from the configured default.
    case 'integer': return Number.isSafeInteger(value) ? null : 'a whole number within ±(2^53 − 1)'
    default: return null // json: any JSON value
  }
}

function describe(value) {
  const text = JSON.stringify(value)
  return text && text.length > 60 ? `${text.slice(0, 60)}…` : String(text)
}

/**
 * Normalize the raw flag objects returned by the API (or read from a file) into
 * the minimal shape the generators need: `{ key, type, default }`.
 *
 * - `type` falls back to `boolean` (matching the server default for flags that
 *   predate multi-variant support).
 * - `default` is the flag's configured `default_value` when present, otherwise
 *   `undefined` (generators substitute a type-appropriate zero value). A `null`
 *   default on a non-`json` flag counts as "not configured".
 * - A default that does not match the flag's type (a string on an integer flag,
 *   an integer beyond ±2^53) is an `invalid_input` error naming every such flag:
 *   generating from it would produce code that does not compile, or a constant
 *   that differs from the server's value.
 * - Archived flags and entries without a string `key` are dropped.
 * - Output is de-duplicated by key and sorted, so generation is deterministic.
 */
export function normalizeFlags(raw) {
  if (!Array.isArray(raw)) {
    throw new CliError('invalid_input', 'Expected an array of flags (the /flags API returns a JSON array).')
  }
  const byKey = new Map()
  for (const f of raw) {
    if (!f || typeof f.key !== 'string' || f.key.length === 0) continue
    if (f.archived_at) continue
    const type = FLAG_TYPES.includes(f.flag_type) ? f.flag_type : 'boolean'
    let value = Object.prototype.hasOwnProperty.call(f, 'default_value') ? f.default_value : undefined
    if (value === null && type !== 'json') value = undefined
    byKey.set(f.key, { key: f.key, type, default: value })
  }
  const flags = [...byKey.values()].sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0))

  const findings = []
  for (const f of flags) {
    if (f.default === undefined) continue
    const expected = defaultMismatch(f.type, f.default)
    if (expected) {
      findings.push({
        id: 'flags.invalid_default',
        severity: 'error',
        flag: f.key,
        message: `Flag "${f.key}" is type ${f.type} but its default_value ${describe(f.default)} is not ${expected}.`,
        action: `Set a default_value for "${f.key}" that matches its type (in Checkgate, or in the --input file).`,
      })
    }
  }
  if (findings.length > 0) {
    const shown = findings.slice(0, 5).map(x => `  - ${x.message}`).join('\n')
    const more = findings.length > 5 ? `\n  … and ${findings.length - 5} more` : ''
    throw new CliError('invalid_input', `${findings.length} flag(s) have a default that does not match their type:\n${shown}${more}`, {
      hint: 'Fix the defaults, then re-run. Generated code from these would not compile or would be wrong.',
      findings,
    })
  }
  return flags
}

/** Split a flag key into lowercase word parts on any non-alphanumeric boundary. */
function words(key) {
  return key
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2') // split camelCase humps
    .split(/[^a-zA-Z0-9]+/)
    .filter(Boolean)
    .map(w => w.toLowerCase())
}

/** `new-dashboard` → `NewDashboard`. Prefixes `_` if it would start with a digit. */
export function pascalCase(key) {
  const id = words(key).map(w => w.charAt(0).toUpperCase() + w.slice(1)).join('')
  return /^[0-9]/.test(id) || id === '' ? `_${id}` : id
}

/** `new-dashboard` → `newDashboard`. Prefixes `_` if it would start with a digit. */
export function camelCase(key) {
  const p = pascalCase(key).replace(/^_/, '')
  const id = p.charAt(0).toLowerCase() + p.slice(1)
  return /^[0-9]/.test(id) || id === '' ? `_${id}` : id
}

/** `new-dashboard` → `NEW_DASHBOARD`. Prefixes `_` if it would start with a digit. */
export function upperSnakeCase(key) {
  const id = words(key).join('_').toUpperCase()
  return /^[0-9]/.test(id) || id === '' ? `_${id}` : id
}

/**
 * Ensure a set of generated identifiers is unique by suffixing collisions with
 * `_2`, `_3`, … Two distinct keys can normalize to the same identifier
 * (`a.b` and `a-b` → `aB`); this keeps the generated code valid.
 */
export function dedupeIdentifiers(names) {
  const seen = new Map()
  return names.map(name => {
    const n = seen.get(name) ?? 0
    seen.set(name, n + 1)
    return n === 0 ? name : `${name}_${n + 1}`
  })
}

// --- Literal emitters ------------------------------------------------------

const DART_KEYWORDS = new Set([
  'assert', 'break', 'case', 'catch', 'class', 'const', 'continue', 'default',
  'do', 'else', 'enum', 'extends', 'false', 'final', 'finally', 'for', 'if',
  'in', 'is', 'new', 'null', 'rethrow', 'return', 'super', 'switch', 'this',
  'throw', 'true', 'try', 'var', 'void', 'while', 'with',
])

/**
 * Names a generated identifier cannot take even though they are not keywords:
 * an enum may not declare `values`, `index` or `hashCode`, the `TypedFlags`
 * methods would be invalid overrides of `Object` members, and the rest would
 * shadow names the generated code refers to.
 */
const DART_RESERVED_MEMBERS = new Set([
  'values', 'index', 'hashCode', 'runtimeType', 'toString', 'noSuchMethod',
  // Names the generated file itself uses: a member called `dynamic`, `bool` or
  // `int` shadows that type inside the class, and `key` is FlagKey's own field.
  'dynamic', 'bool', 'int', 'key',
])

/**
 * Make an identifier safe for Dart. A leading underscore makes a Dart
 * declaration library-*private*, which would hide a generated enum member or
 * method from consumers, so swap it for an `f` (flag) prefix. Reserved words
 * and reserved member names get a `Flag` suffix.
 */
export function safeDartIdent(name) {
  const n = name.startsWith('_') ? `f${name.slice(1)}` : name
  return DART_KEYWORDS.has(n) || DART_RESERVED_MEMBERS.has(n) ? `${n}Flag` : n
}

/** Zero value for a flag type when it has no configured default. */
export function zeroValue(type) {
  switch (type) {
    case 'string': return ''
    case 'integer': return 0
    case 'json': return null
    default: return false
  }
}

/** TypeScript literal from a JSON value. */
export function tsLiteral(value) {
  return JSON.stringify(value)
}

/** Dart literal from a JSON value. `$` is escaped to avoid string interpolation; control characters are escaped. */
export function dartLiteral(value) {
  if (typeof value === 'string') {
    const body = value
      .replace(/\\/g, '\\\\')
      .replace(/'/g, "\\'")
      .replace(/\$/g, '\\$')
      .replace(/\n/g, '\\n')
      .replace(/\r/g, '\\r')
      .replace(/\t/g, '\\t')
      // Any other control character: a raw CR or similar inside a one-line
      // Dart string literal does not compile.
      .replace(/[\u0000-\u001f\u007f]/g, c => `\\u{${c.charCodeAt(0).toString(16)}}`)
    return `'${body}'`
  }
  // JSON object/array/number/bool/null literals are all valid Dart literal
  // syntax; only `$` inside nested strings needs escaping.
  return JSON.stringify(value).replace(/\$/g, '\\$')
}

/**
 * Rust literal from a JSON value. `kind` is the flag's Rust value kind:
 * `'bool'`, `'i64'`, `'str'` (a plain string flag), or `'json'` (raw JSON text).
 * String and JSON values become raw string literals with enough `#` hashes to
 * never collide with their content.
 */
export function rustLiteral(value, kind) {
  if (kind === 'bool') return value ? 'true' : 'false'
  if (kind === 'i64') return `${Math.trunc(Number(value))}`
  const text = kind === 'str' ? String(value) : JSON.stringify(value)
  // A raw string may contain anything except a bare CR; fall back to an
  // escaped literal for that (rare) case.
  if (text.includes('\r')) return rustEscapedLiteral(text)
  let hashes = '#'
  while (text.includes(`"${hashes}`)) hashes += '#'
  return `r${hashes}"${text}"${hashes}`
}

/**
 * Ordinary (escaped) Rust string literal. Plain text comes out exactly as
 * `JSON.stringify` would write it, but control characters use Rust's escapes
 * (`JSON.stringify` writes `\f`, `\b`, `\u0001`, which Rust rejects).
 */
export function rustEscapedLiteral(text) {
  let out = '"'
  for (const ch of text) {
    const c = ch.codePointAt(0)
    if (ch === '\\') out += '\\\\'
    else if (ch === '"') out += '\\"'
    else if (ch === '\n') out += '\\n'
    else if (ch === '\r') out += '\\r'
    else if (ch === '\t') out += '\\t'
    else if (c < 0x20 || c === 0x7f) out += `\\u{${c.toString(16)}}`
    else out += ch
  }
  return `${out}"`
}
