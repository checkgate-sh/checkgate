import { test } from 'node:test'
import assert from 'node:assert/strict'
import { ConfigError, DEFAULT_TIMEOUT_MS, loadConfig, redactUrl } from '../src/config.js'

const base = { CHECKGATE_URL: 'https://flags.example.com/', CHECKGATE_TOKEN: 'pat_x' }

test('defaults: read-only, propose guard on, 10 s timeout, trailing slash trimmed', () => {
  const c = loadConfig(base)
  assert.equal(c.url, 'https://flags.example.com')
  assert.equal(c.write, false)
  assert.equal(c.requirePropose, true)
  assert.equal(c.timeoutMs, DEFAULT_TIMEOUT_MS)
  assert.equal(DEFAULT_TIMEOUT_MS, 10_000)
  assert.ok(Object.isFrozen(c))
})

test('write tools and the guard are explicit switches', () => {
  assert.equal(loadConfig({ ...base, CHECKGATE_MCP_WRITE: '1' }).write, true)
  assert.equal(loadConfig({ ...base, CHECKGATE_MCP_WRITE: 'true' }).write, true)
  assert.equal(loadConfig({ ...base, CHECKGATE_MCP_WRITE: '0' }).write, false)
  assert.equal(loadConfig({ ...base, CHECKGATE_MCP_REQUIRE_PROPOSE: '0' }).requirePropose, false)
  assert.throws(() => loadConfig({ ...base, CHECKGATE_MCP_WRITE: 'maybe' }), ConfigError)
  assert.throws(() => loadConfig({ ...base, CHECKGATE_MCP_REQUIRE_PROPOSE: 'off-ish' }), ConfigError)
})

test('a URL with credentials is refused without repeating them', () => {
  for (const url of ['https://admin:hunter2@flags.example.com', 'https://hunter2@flags.example.com']) {
    let err
    try { loadConfig({ ...base, CHECKGATE_URL: url }) } catch (e) { err = e }
    assert.ok(err instanceof ConfigError, url)
    assert.match(err.message, /must not contain credentials/)
    assert.ok(!err.message.includes('hunter2'), err.message)
  }
})

test('other bad URLs', () => {
  for (const url of [undefined, '', 'not a url', 'ftp://flags.example.com', 'https://flags.example.com/?token=abc', 'https://flags.example.com/#x']) {
    let err
    try { loadConfig({ ...base, CHECKGATE_URL: url }) } catch (e) { err = e }
    assert.ok(err instanceof ConfigError, String(url))
    assert.ok(!err.message.includes('token=abc'), err.message)
  }
})

test('token is required and must be header-safe; it is never in the message', () => {
  assert.throws(() => loadConfig({ ...base, CHECKGATE_TOKEN: '' }), /CHECKGATE_TOKEN is not set/)
  for (const t of ['pat_abc\r\nX-Evil: 1', 'pat with space', 'x'.repeat(513)]) {
    let err
    try { loadConfig({ ...base, CHECKGATE_TOKEN: t }) } catch (e) { err = e }
    assert.ok(err instanceof ConfigError)
    assert.ok(!err.message.includes(t.slice(0, 8)) || t.startsWith('x'), err.message)
  }
})

test('numeric settings are bounded', () => {
  assert.equal(loadConfig({ ...base, CHECKGATE_TIMEOUT_MS: '2500' }).timeoutMs, 2500)
  for (const v of ['0', '99', '120001', '-5', '1e3', 'abc']) {
    assert.throws(() => loadConfig({ ...base, CHECKGATE_TIMEOUT_MS: v }), ConfigError, v)
  }
  assert.throws(() => loadConfig({ ...base, CHECKGATE_MCP_MAX_RESPONSE_BYTES: '10' }), ConfigError)
})

test('redactUrl strips userinfo, query and fragment', () => {
  assert.equal(redactUrl('https://u:p@h.example/x?t=1#y'), 'https://h.example/x')
  assert.equal(redactUrl('::nope'), '<unparseable URL>')
})

test('plain http:// is flagged only for hosts other than this machine', async () => {
  const { cleartextToRemote } = await import('../src/index.js')
  assert.equal(cleartextToRemote('http://flags.example.com'), true)
  assert.equal(cleartextToRemote('http://10.0.0.5:3000'), true)
  for (const local of ['http://localhost:3000', 'http://127.0.0.1:3000', 'http://[::1]:3000', 'https://flags.example.com']) {
    assert.equal(cleartextToRemote(local), false, local)
  }
})
