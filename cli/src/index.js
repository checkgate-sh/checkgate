// Orchestrates the `checkgate` CLI: parse args, load flags, generate code, then
// write it (or, with --check, compare it with what is on disk) and report.

import { readFileSync } from 'node:fs'
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { parseArgs, validateOptions } from './args.js'
import { loadFlags, DEFAULT_TIMEOUT_MS } from './source.js'
import { normalizeFlags } from './util.js'
import { generate, resolveLang, LANG_EXT, SUPPORTED_LANGS } from './generate.js'
import { CliError, EXIT, redactUrl, usageError } from './errors.js'
import { writeFileAtomic } from './write.js'

/** Read from package.json so `--version` cannot drift from the published package. */
export const VERSION = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')).version

/** Version of the `--json` report shape. Bumped only on an incompatible change. */
export const REPORT_SCHEMA_VERSION = 1

const HELP = `checkgate — type-safe flag accessors from your Checkgate flags

Usage:
  checkgate typegen --lang <langs> [options]

Languages (comma-separated for more than one):
  ts | typescript, dart, rust | rs

Source (choose one):
  --input, -i <file>     Read flags from a local JSON file (a /flags export)
  --url, -u <url>        Checkgate server base URL        (env: CHECKGATE_URL)
  --env, -e <env-id>     Environment id to read flags from (env: CHECKGATE_ENV)
  --token, -t <token>    Personal access token / SDK key   (env: CHECKGATE_TOKEN)
  --timeout <seconds>    Per-request timeout, default 30   (env: CHECKGATE_TIMEOUT)

Output:
  --out, -o <file>       Write to a file (single language). Omit to print to stdout.
  --out-dir <dir>        Write flags.<ext> per language (required for multiple languages).

CI and automation:
  --check                Write nothing; exit 3 if any output file is missing or
                         out of date. Needs --out or --out-dir.
  --json                 Print one JSON result object on stdout (and nothing else).
                         Needs --out or --out-dir.

Other:
  --help, -h             Show this help
  --version              Show version

Exit codes:
  0 success · 1 operational failure (network, auth, server, bad data, I/O)
  2 usage error · 3 --check found missing or out-of-date files

Examples:
  checkgate typegen --lang ts --url http://localhost:3000 --env <id> --token <pat> -o flags.ts
  checkgate typegen --lang ts,dart,rust --input flags.json --out-dir ./generated
  checkgate typegen --lang ts --input flags.json -o src/flags.ts --check --json
`

/**
 * The `// Source:` header line records where flags were read from. It is
 * ignored by --check, so files generated from the server compare equal to the
 * same flags read from an exported file in CI.
 */
const SOURCE_LINE = /^\/\/ Source: .*$/m

function comparable(code) {
  return code.replace(SOURCE_LINE, '')
}

function parseTimeout(options) {
  const raw = options.timeout ?? process.env.CHECKGATE_TIMEOUT
  if (raw === undefined || raw === '') return DEFAULT_TIMEOUT_MS
  const seconds = Number(raw)
  if (!Number.isFinite(seconds) || seconds <= 0) {
    throw usageError(`--timeout must be a positive number of seconds (got "${raw}").`)
  }
  return Math.round(seconds * 1000)
}

function resolveLangs(options) {
  const names = String(options.lang ?? '').split(',').map(s => s.trim()).filter(Boolean)
  if (names.length === 0) throw usageError('--lang is required (ts, dart, rust — comma-separated for more).')
  const langs = []
  for (const name of names) {
    const lang = resolveLang(name)
    if (!lang) throw usageError(`Unsupported language "${name}". Supported: ${SUPPORTED_LANGS.join(', ')}.`)
    if (!langs.includes(lang)) langs.push(lang)
  }
  return langs
}

/**
 * Do the work and return a result object; throws CliError on a classified
 * failure. Never writes to stdout/stderr itself except generated code when no
 * output file was given.
 */
async function typegen(options, report, io) {
  const langs = resolveLangs(options)
  const outDir = options['out-dir']
  const out = options.out
  if (langs.length > 1 && !outDir) throw usageError('Generating multiple languages requires --out-dir.')
  if ((options.check || options.json) && !outDir && !out) {
    throw usageError(`--${options.check ? 'check' : 'json'} needs --out or --out-dir (stdout is reserved for ${options.json ? 'the JSON result' : 'generated code'}).`)
  }
  const timeoutMs = parseTimeout(options)

  const source = {
    input: options.input,
    url: options.url ?? process.env.CHECKGATE_URL,
    env: options.env ?? process.env.CHECKGATE_ENV,
    token: options.token ?? process.env.CHECKGATE_TOKEN,
    timeoutMs,
  }
  report.source = source.input
    ? { type: 'file', location: source.input }
    : { type: 'server', location: source.url ? redactUrl(source.url) : null, environment: source.env ?? null }

  const flags = normalizeFlags(await loadFlags(source))
  report.flag_count = flags.length

  const sourceLabel = source.input ? source.input : `${redactUrl(source.url)} (env ${source.env})`
  const meta = { source: sourceLabel, count: flags.length }

  for (const lang of langs) {
    const code = generate(lang, flags, meta)
    const path = outDir ? join(outDir, `flags.${LANG_EXT[lang]}`) : out
    if (!path) {
      io.stdout.write(code)
      continue
    }
    const file = { path, language: lang, status: null }
    report.files.push(file)

    let existing = null
    try {
      existing = await readFile(path, 'utf8')
    } catch (e) {
      if (e.code !== 'ENOENT') throw new CliError('io', `Could not read ${path}: ${e.message}`)
    }

    if (options.check) {
      if (existing === null) {
        file.status = 'missing'
        report.findings.push({
          id: 'typegen.missing',
          severity: 'error',
          path,
          message: `${path} does not exist.`,
          action: 'Run the same command without --check to generate it, then commit it.',
        })
      } else if (comparable(existing) !== comparable(code)) {
        file.status = 'out_of_date'
        report.findings.push({
          id: 'typegen.out_of_date',
          severity: 'error',
          path,
          message: `${path} does not match the current flag definitions (${flags.length} flag(s)).`,
          action: 'Run the same command without --check to regenerate it, then commit the result.',
        })
      } else {
        file.status = 'up_to_date'
      }
      continue
    }

    if (existing === code) {
      // Leave an identical file alone so its mtime does not trigger rebuilds.
      file.status = 'unchanged'
    } else {
      try {
        await writeFileAtomic(path, code)
      } catch (e) {
        throw new CliError('io', `Could not write ${path}: ${e.message}`)
      }
      file.status = 'written'
    }
  }

  if (report.findings.length > 0) {
    report.status = 'out_of_date'
    return EXIT.OUT_OF_DATE
  }
  report.status = 'ok'
  return EXIT.OK
}

function humanSummary(report, io) {
  for (const f of report.files) {
    const line = {
      written: `Wrote ${report.flag_count} flag(s) → ${f.path}`,
      unchanged: `Unchanged (${report.flag_count} flag(s)) → ${f.path}`,
      up_to_date: `Up to date: ${f.path}`,
      out_of_date: `Out of date: ${f.path}`,
      missing: `Missing: ${f.path}`,
    }[f.status]
    if (line) io.stderr.write(`${line}\n`)
  }
  if (report.status === 'out_of_date') {
    io.stderr.write('Generated flag files are stale. Re-run without --check to update them.\n')
  }
}

/**
 * Run the CLI. Returns the process exit code; never throws (an unexpected
 * exception is reported as an `internal` error, exit 1).
 *
 * @param {string[]} argv arguments without `node` and the script path
 * @param {{ stdout: { write(s: string): unknown }, stderr: { write(s: string): unknown } }} [io]
 */
export async function run(argv, io = { stdout: process.stdout, stderr: process.stderr }) {
  const started = Date.now()
  const parsed = parseArgs(argv)
  const { command, options } = parsed
  const json = options.json === true

  const report = {
    schema_version: REPORT_SCHEMA_VERSION,
    tool: { name: '@checkgate/cli', version: VERSION },
    command: command ?? null,
    mode: options.check ? 'check' : 'write',
    status: 'error',
    exit_code: EXIT.ERROR,
    source: null,
    flag_count: null,
    files: [],
    findings: [],
    error: null,
    duration_ms: 0,
  }

  let code
  try {
    if (options.version) {
      io.stdout.write(`${VERSION}\n`)
      return EXIT.OK
    }
    if (options.help) {
      io.stdout.write(HELP)
      return EXIT.OK
    }
    if (!command) {
      if (json) throw usageError('No command given. The only command is `typegen`.')
      io.stderr.write(HELP)
      return EXIT.USAGE
    }
    if (command !== 'typegen') throw usageError(`Unknown command: ${command}. The only command is \`typegen\`.`)
    validateOptions(parsed)
    code = await typegen(options, report, io)
  } catch (e) {
    const err = e instanceof CliError
      ? e
      : new CliError('internal', `Unexpected error: ${e?.stack ?? e}`, {
          hint: 'This is a bug in @checkgate/cli — please report it with the command you ran.',
        })
    report.status = 'error'
    report.error = { code: err.code, message: err.message, ...(err.hint ? { hint: err.hint } : {}) }
    report.findings.push(...err.findings)
    code = err.exitCode
    // Human-readable error on stderr in both modes, so a log reader sees it.
    io.stderr.write(`Error: ${err.message}\n${err.hint ? `Hint: ${err.hint}\n` : ''}`)
  }

  report.exit_code = code
  report.duration_ms = Date.now() - started
  if (json) io.stdout.write(`${JSON.stringify(report, null, 2)}\n`)
  // The summary goes to stderr in both modes: with --json redirected to a file,
  // the CI log still says which files were stale.
  humanSummary(report, io)
  return code
}
