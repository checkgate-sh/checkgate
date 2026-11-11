// Minimal, dependency-free argument parser for the `checkgate` CLI.

import { usageError } from './errors.js'

const ALIASES = { l: 'lang', o: 'out', u: 'url', e: 'env', t: 'token', i: 'input', h: 'help' }
const BOOLEAN_FLAGS = new Set(['help', 'version', 'check', 'json'])
/** Options that take a value. A bare `--out` with nothing after it is a usage error. */
const VALUE_FLAGS = new Set(['lang', 'out', 'out-dir', 'url', 'env', 'token', 'input', 'timeout'])
const KNOWN_FLAGS = new Set([...BOOLEAN_FLAGS, ...VALUE_FLAGS])

/**
 * Parse argv (without the leading `node` / script entries) into
 * `{ command, options }`. Supports `--flag value`, `--flag=value`, short
 * aliases, and boolean flags (`--help`, `--version`, `--check`, `--json`).
 *
 * Parsing never fails; `validateOptions` reports unknown or malformed options
 * so the caller can decide how to surface them.
 */
export function parseArgs(argv) {
  const options = {}
  let command = null
  const positional = []

  for (let i = 0; i < argv.length; i++) {
    let arg = argv[i]
    if (!arg.startsWith('-')) {
      if (command === null) command = arg
      else positional.push(arg)
      continue
    }

    let key = arg.replace(/^-+/, '')
    let value

    const eq = key.indexOf('=')
    if (eq !== -1) {
      value = key.slice(eq + 1)
      key = key.slice(0, eq)
    }

    key = ALIASES[key] ?? key

    if (BOOLEAN_FLAGS.has(key)) {
      options[key] = true
      continue
    }

    if (value === undefined) {
      const next = argv[i + 1]
      if (next === undefined || next.startsWith('-')) {
        // A lone `--flag` with no value: treat as boolean true.
        options[key] = true
        continue
      }
      value = next
      i++
    }
    options[key] = value
  }

  return { command, options, positional }
}

/** Levenshtein distance, for "did you mean" suggestions on short option names. */
function distance(a, b) {
  const row = Array.from({ length: b.length + 1 }, (_, j) => j)
  for (let i = 1; i <= a.length; i++) {
    let prev = row[0]
    row[0] = i
    for (let j = 1; j <= b.length; j++) {
      const tmp = row[j]
      row[j] = Math.min(row[j] + 1, row[j - 1] + 1, prev + (a[i - 1] === b[j - 1] ? 0 : 1))
      prev = tmp
    }
  }
  return row[b.length]
}

function suggest(name) {
  let best = null
  let bestD = Infinity
  for (const k of KNOWN_FLAGS) {
    const d = distance(name, k)
    if (d < bestD) { best = k; bestD = d }
  }
  return bestD <= 2 ? best : null
}

/**
 * Reject invocations that would otherwise be silently misread: an unknown
 * option (a typo like `--out-dirr` used to print to stdout and exit 0), an
 * option that needs a value but has none, a stray positional argument, or two
 * options that contradict each other. Throws a `usage` CliError (exit 2).
 */
export function validateOptions({ options, positional }) {
  for (const key of Object.keys(options)) {
    if (!KNOWN_FLAGS.has(key)) {
      const s = suggest(key)
      throw usageError(`Unknown option --${key}.${s ? ` Did you mean --${s}?` : ''}`)
    }
    if (VALUE_FLAGS.has(key) && (options[key] === true || options[key] === '')) {
      throw usageError(`Option --${key} needs a value.`)
    }
  }
  if (positional.length > 0) {
    throw usageError(`Unexpected argument "${positional[0]}". Options take the form --name value.`)
  }
  if (options.out !== undefined && options['out-dir'] !== undefined) {
    throw usageError('--out and --out-dir cannot be used together. Use --out for one language, --out-dir for several.')
  }
  if (options.input !== undefined && (options.url !== undefined || options.env !== undefined)) {
    throw usageError('--input reads a local file, so --url / --env would be ignored. Pass one source, not both.')
  }
}
