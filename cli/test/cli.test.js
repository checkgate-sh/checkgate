// End-to-end tests of the `checkgate` binary: exit codes, stdout/stderr
// separation, --check, --json, timeouts, signals and file writes. Each test
// spawns the real CLI. Nothing waits on a fixed sleep — tests react to events
// (a request arriving, the process exiting).

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import http from 'node:http'
import { mkdtemp, readFile, writeFile, readdir, stat, mkdir, copyFile, rm } from 'node:fs/promises'
import { readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { writeFileAtomic } from '../src/write.js'

const BIN = fileURLToPath(new URL('../bin/checkgate.js', import.meta.url))
const FIXTURE = fileURLToPath(new URL('./fixtures/flags.json', import.meta.url))
const PKG = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'))
const FLAGS = JSON.parse(readFileSync(FIXTURE, 'utf8'))

// The CLI must not pick up the developer's or CI's own server settings.
const CLEAN_ENV = Object.fromEntries(Object.entries(process.env).filter(([k]) => !k.startsWith('CHECKGATE_')))

function cli(args, { env = {}, onSpawn } = {}) {
  return new Promise(resolve => {
    const child = spawn(process.execPath, [BIN, ...args], { env: { ...CLEAN_ENV, ...env } })
    let stdout = ''
    let stderr = ''
    child.stdout.on('data', d => { stdout += d })
    child.stderr.on('data', d => { stderr += d })
    child.on('close', (code, signal) => resolve({ code, signal, stdout, stderr }))
    onSpawn?.(child)
  })
}

async function tmp() {
  return mkdtemp(join(tmpdir(), 'checkgate-cli-'))
}

/** Start a server; resolves with its base URL and a close function. */
async function server(handler) {
  const srv = http.createServer(handler)
  await new Promise(r => srv.listen(0, '127.0.0.1', r))
  const { port } = srv.address()
  return {
    url: `http://127.0.0.1:${port}`,
    close: () => { srv.closeAllConnections(); return new Promise(r => srv.close(r)) },
  }
}

function jsonServer(status, body, type = 'application/json') {
  return server((req, res) => {
    res.writeHead(status, { 'content-type': type })
    res.end(typeof body === 'string' ? body : JSON.stringify(body))
  })
}

const SERVER_ARGS = (url) => ['--url', url, '--env', 'env-1', '--token', 'pat-secret']

// --- invocation & usage errors (exit 2) -------------------------------------

test('--version prints the package.json version', async () => {
  const r = await cli(['--version'])
  assert.equal(r.code, 0)
  assert.equal(r.stdout.trim(), PKG.version)
})

test('unknown option is a usage error with a suggestion, and writes nothing', async () => {
  const dir = await tmp()
  const r = await cli(['typegen', '-l', 'ts', '-i', FIXTURE, '--out-dirr', dir])
  assert.equal(r.code, 2)
  assert.equal(r.stdout, '')
  assert.match(r.stderr, /Unknown option --out-dirr\. Did you mean --out-dir\?/)
  assert.deepEqual(await readdir(dir), [])
})

for (const [name, args, pattern] of [
  ['conflicting --out and --out-dir', ['-o', 'a.ts', '--out-dir', 'd'], /cannot be used together/],
  ['option without a value', ['-o'], /--out needs a value/],
  ['stray positional argument', ['ts'], /Unexpected argument "ts"/],
  ['--check without an output path', ['--check'], /--check needs --out or --out-dir/],
  ['--json without an output path', ['--json'], /--json needs --out or --out-dir/],
  ['non-numeric --timeout', ['-o', 'a.ts', '--timeout', 'soon'], /--timeout must be a positive number/],
  ['unsupported language', ['-l', 'go'], /Unsupported language "go"/],
]) {
  test(`usage error: ${name} → exit 2`, async () => {
    const lang = args.includes('-l') ? [] : ['-l', 'ts']
    const r = await cli(['typegen', ...lang, '-i', FIXTURE, ...args])
    assert.equal(r.code, 2, r.stderr)
    assert.match(r.stderr, pattern)
  })
}

test('no command prints help on stderr and exits 2', async () => {
  const r = await cli([])
  assert.equal(r.code, 2)
  assert.equal(r.stdout, '')
  assert.match(r.stderr, /Usage:/)
})

test('--input together with --url is a usage error', async () => {
  const r = await cli(['typegen', '-l', 'ts', '-i', FIXTURE, '--url', 'http://x', '-o', 'a.ts'])
  assert.equal(r.code, 2)
  assert.match(r.stderr, /Pass one source, not both/)
})

test('CHECKGATE_URL in the environment does not conflict with --input', async () => {
  const dir = await tmp()
  const r = await cli(['typegen', '-l', 'ts', '-i', FIXTURE, '-o', join(dir, 'f.ts')], { env: { CHECKGATE_URL: 'http://x' } })
  assert.equal(r.code, 0, r.stderr)
})

// --- writing ------------------------------------------------------------------

test('stdout mode is unchanged: code on stdout, nothing on stderr', async () => {
  const r = await cli(['typegen', '-l', 'ts', '-i', FIXTURE])
  assert.equal(r.code, 0)
  assert.match(r.stdout, /export type FlagKey =/)
  assert.equal(r.stderr, '')
})

test('repeated runs produce identical files and leave an unchanged file untouched', async () => {
  const dir = await tmp()
  const args = ['typegen', '-l', 'ts,dart,rust', '-i', FIXTURE, '--out-dir', dir, '--json']
  const first = JSON.parse((await cli(args)).stdout)
  assert.deepEqual(first.files.map(f => f.status), ['written', 'written', 'written'])
  const before = await readFile(join(dir, 'flags.ts'), 'utf8')
  const mtime = (await stat(join(dir, 'flags.ts'))).mtimeMs

  const second = JSON.parse((await cli(args)).stdout)
  assert.deepEqual(second.files.map(f => f.status), ['unchanged', 'unchanged', 'unchanged'])
  assert.equal(await readFile(join(dir, 'flags.ts'), 'utf8'), before)
  assert.equal((await stat(join(dir, 'flags.ts'))).mtimeMs, mtime)
  // No temp files are left behind.
  assert.deepEqual((await readdir(dir)).sort(), ['flags.dart', 'flags.rs', 'flags.ts'])
})

test('writeFileAtomic removes its temp file when the final rename fails', async () => {
  const dir = await tmp()
  const target = join(dir, 'occupied')
  await mkdir(target)
  await writeFile(join(target, 'keep'), 'x') // a non-empty directory cannot be replaced by a file
  await assert.rejects(writeFileAtomic(target, 'data'))
  assert.deepEqual(await readdir(dir), ['occupied'])
})

test('an unwritable output path is an io error (exit 1)', async () => {
  const dir = await tmp()
  const blocker = join(dir, 'file')
  await writeFile(blocker, 'x')
  const r = await cli(['typegen', '-l', 'ts', '-i', FIXTURE, '-o', join(blocker, 'flags.ts'), '--json'])
  assert.equal(r.code, 1)
  assert.equal(JSON.parse(r.stdout).error.code, 'io')
})

// --- --check ------------------------------------------------------------------

test('--check: up to date → exit 0, out of date → exit 3, missing → exit 3; never writes', async () => {
  const dir = await tmp()
  const base = ['typegen', '-l', 'ts,rust', '-i', FIXTURE, '--out-dir', dir]
  assert.equal((await cli(base)).code, 0)

  const ok = await cli([...base, '--check', '--json'])
  assert.equal(ok.code, 0, ok.stderr)
  const okReport = JSON.parse(ok.stdout)
  assert.equal(okReport.status, 'ok')
  assert.equal(okReport.mode, 'check')
  assert.deepEqual(okReport.files.map(f => f.status), ['up_to_date', 'up_to_date'])
  assert.deepEqual(okReport.findings, [])

  const stale = '// edited by hand\n'
  await writeFile(join(dir, 'flags.ts'), stale)
  await rm(join(dir, 'flags.rs'))
  const bad = await cli([...base, '--check', '--json'])
  assert.equal(bad.code, 3)
  const report = JSON.parse(bad.stdout)
  assert.equal(report.status, 'out_of_date')
  assert.equal(report.exit_code, 3)
  assert.deepEqual(report.files.map(f => [f.language, f.status]), [['typescript', 'out_of_date'], ['rust', 'missing']])
  assert.deepEqual(report.findings.map(f => f.id), ['typegen.out_of_date', 'typegen.missing'])
  // The CI log (stderr) names the stale files even when stdout is the report.
  assert.match(bad.stderr, /Out of date: .*flags\.ts/)
  assert.ok(report.findings.every(f => f.severity === 'error' && f.path && f.action))
  // --check wrote nothing.
  assert.equal(await readFile(join(dir, 'flags.ts'), 'utf8'), stale)
  assert.deepEqual((await readdir(dir)).sort(), ['flags.ts'])

  const human = await cli([...base, '--check'])
  assert.equal(human.code, 3)
  assert.equal(human.stdout, '')
  assert.match(human.stderr, /Out of date: .*flags\.ts/)
  assert.match(human.stderr, /Missing: .*flags\.rs/)
})

test('--check ignores the Source header, so a file generated from the server checks against an export', async () => {
  const dir = await tmp()
  const srv = await jsonServer(200, FLAGS)
  try {
    const out = join(dir, 'flags.ts')
    assert.equal((await cli(['typegen', '-l', 'ts', ...SERVER_ARGS(srv.url), '-o', out])).code, 0)
    const copy = join(dir, 'export.json')
    await copyFile(FIXTURE, copy)
    const r = await cli(['typegen', '-l', 'ts', '-i', copy, '-o', out, '--check'])
    assert.equal(r.code, 0, r.stderr)
  } finally {
    await srv.close()
  }
})

// --- --json report --------------------------------------------------------------

test('--json: stdout is exactly one JSON document with the documented fields', async () => {
  const dir = await tmp()
  const r = await cli(['typegen', '-l', 'ts', '-i', FIXTURE, '-o', join(dir, 'f.ts'), '--json'])
  assert.equal(r.code, 0)
  const report = JSON.parse(r.stdout) // throws if anything else is on stdout
  assert.deepEqual(Object.keys(report), [
    'schema_version', 'tool', 'command', 'mode', 'status', 'exit_code', 'source',
    'flag_count', 'files', 'findings', 'error', 'duration_ms',
  ])
  assert.equal(report.schema_version, 1)
  assert.deepEqual(report.tool, { name: '@checkgate/cli', version: PKG.version })
  assert.equal(report.status, 'ok')
  assert.equal(report.flag_count, 6)
  assert.deepEqual(report.source, { type: 'file', location: FIXTURE })
  assert.equal(report.error, null)
  assert.equal(typeof report.duration_ms, 'number')
})

test('--json reports usage errors as JSON too', async () => {
  const r = await cli(['typegen', '--json', '--bogus'])
  assert.equal(r.code, 2)
  const report = JSON.parse(r.stdout)
  assert.equal(report.error.code, 'usage')
  assert.equal(report.exit_code, 2)
  assert.match(r.stderr, /Unknown option --bogus/)
})

test('invalid defaults → exit 1, invalid_input, one finding per flag', async () => {
  const dir = await tmp()
  const input = join(dir, 'bad.json')
  await writeFile(input, JSON.stringify([
    { key: 'limit', flag_type: 'integer', default_value: 'ten' },
    { key: 'on', flag_type: 'boolean', default_value: 'yes' },
    { key: 'fine', flag_type: 'string', default_value: 'ok' },
  ]))
  const r = await cli(['typegen', '-l', 'rust', '-i', input, '-o', join(dir, 'f.rs'), '--json'])
  assert.equal(r.code, 1)
  const report = JSON.parse(r.stdout)
  assert.equal(report.error.code, 'invalid_input')
  assert.deepEqual(report.findings.map(f => [f.id, f.flag]), [['flags.invalid_default', 'limit'], ['flags.invalid_default', 'on']])
  assert.deepEqual(await readdir(dir), ['bad.json']) // nothing generated
})

// --- server failures are classified -------------------------------------------

for (const [name, status, body, type, code, pattern] of [
  ['401', 401, { error: 'no' }, 'application/json', 'auth', /rejected the token \(HTTP 401\)/],
  ['404', 404, { error: 'no' }, 'application/json', 'not_found', /Environment not found/],
  ['500 with an error body', 500, { error: 'database unavailable' }, 'application/json', 'server', /HTTP 500\): database unavailable/],
  ['HTML instead of JSON', 200, '<html>SSO login</html>', 'text/html', 'bad_response', /text\/html content that is not JSON/],
  ['JSON that is not an array', 200, { flags: [] }, 'application/json', 'bad_response', /not a JSON array/],
]) {
  test(`server ${name} → exit 1, error.code ${code}`, async () => {
    const srv = await jsonServer(status, body, type)
    try {
      const dir = await tmp()
      const r = await cli(['typegen', '-l', 'ts', ...SERVER_ARGS(srv.url), '-o', join(dir, 'f.ts'), '--json'])
      assert.equal(r.code, 1)
      const report = JSON.parse(r.stdout)
      assert.equal(report.error.code, code)
      assert.match(report.error.message, pattern)
      assert.ok(report.error.hint)
      assert.doesNotMatch(r.stdout + r.stderr, /pat-secret/)
    } finally {
      await srv.close()
    }
  })
}

test('connection refused → network error', async () => {
  const srv = await server(() => {})
  const { url } = srv
  await srv.close() // the port is now closed
  const r = await cli(['typegen', '-l', 'ts', ...SERVER_ARGS(url), '-o', 'unused.ts', '--json'])
  assert.equal(r.code, 1)
  assert.equal(JSON.parse(r.stdout).error.code, 'network')
})

test('a server that never answers is a timeout error, not a hang', async () => {
  const srv = await server(() => { /* accept, never respond */ })
  try {
    const r = await cli(['typegen', '-l', 'ts', ...SERVER_ARGS(srv.url), '-o', 'unused.ts', '--timeout', '0.3', '--json'])
    assert.equal(r.code, 1)
    const report = JSON.parse(r.stdout)
    assert.equal(report.error.code, 'timeout')
    assert.match(report.error.message, /within 0\.3 s/)
  } finally {
    await srv.close()
  }
})

test('CHECKGATE_TIMEOUT applies when --timeout is not given', async () => {
  const srv = await server(() => {})
  try {
    const r = await cli(['typegen', '-l', 'ts', ...SERVER_ARGS(srv.url), '-o', 'unused.ts', '--json'], { env: { CHECKGATE_TIMEOUT: '0.2' } })
    assert.equal(JSON.parse(r.stdout).error.code, 'timeout')
  } finally {
    await srv.close()
  }
})

test('credentials in --url are refused without being echoed', async () => {
  // fetch() rejects such URLs with an error that quotes the URL, which the
  // previous CLI printed — leaking the password into CI logs.
  const r = await cli(['typegen', '-l', 'ts', '--url', 'http://admin:hunter2@127.0.0.1:9', '--env', 'e', '--token', 'pat-secret', '-o', 'unused.ts', '--json'])
  assert.equal(r.code, 2)
  assert.equal(JSON.parse(r.stdout).error.code, 'usage')
  assert.match(r.stderr, /must not contain credentials/)
  assert.doesNotMatch(r.stdout + r.stderr, /hunter2|admin:|pat-secret/)
})

test('the generated header and report name the server without its query string', async () => {
  const srv = await jsonServer(200, FLAGS)
  try {
    const dir = await tmp()
    const out = join(dir, 'f.ts')
    const r = await cli(['typegen', '-l', 'ts', '--url', `${srv.url}/`, '--env', 'env-1', '--token', 'pat-secret', '-o', out, '--json'])
    assert.equal(r.code, 0, r.stderr)
    assert.match(await readFile(out, 'utf8'), new RegExp(`^// Source: ${srv.url} \\(env env-1\\)$`, 'm'))
    assert.equal(JSON.parse(r.stdout).source.location, srv.url)
    assert.doesNotMatch(r.stdout + r.stderr + await readFile(out, 'utf8'), /pat-secret/)
  } finally {
    await srv.close()
  }
})

// --- interruption -------------------------------------------------------------

test('SIGINT while waiting on the server exits 130 and leaves no files', async () => {
  const dir = await tmp()
  let child
  let release
  const requested = new Promise(r => { release = r })
  const srv = await server(() => release())
  try {
    const done = cli(['typegen', '-l', 'ts', ...SERVER_ARGS(srv.url), '-o', join(dir, 'f.ts')], { onSpawn: c => { child = c } })
    await requested // the CLI is now blocked on the request
    child.kill('SIGINT')
    const r = await done
    assert.equal(r.code, 130)
    assert.match(r.stderr, /Interrupted \(SIGINT\)/)
    assert.deepEqual(await readdir(dir), [])
  } finally {
    await srv.close()
  }
})
