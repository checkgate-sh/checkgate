// The real process over stdio: protocol handshake, tool registration by
// environment, a tool call, and lifecycle (stdin close, SIGTERM).

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { E_OPEN, TOKENS, startFakeApi } from './fake-api.js'
import { initialize, start, within } from './stdio-client.js'

test('stdio: initialize, tools/list (read-only), tools/call, then stdin close exits 0 promptly', async () => {
  const fake = await startFakeApi()
  try {
    const p = start({ CHECKGATE_URL: fake.url, CHECKGATE_TOKEN: TOKENS.propose })
    await initialize(p)
    const list = await p.send('tools/list', {})
    const names = list.result.tools.map((t) => t.name)
    assert.ok(names.includes('get_flag'))
    assert.ok(!names.includes('propose_flag_change'), 'write tools absent by default')
    const call = await p.send('tools/call', { name: 'get_flag', arguments: { environment_id: E_OPEN, key: 'checkout' } })
    assert.equal(call.result.structuredContent.data.flag.key, 'checkout')
    const t0 = Date.now()
    p.child.stdin.end()
    const { code } = await within(p.exited, 3000, 'exit after stdin close')
    assert.equal(code, 0)
    assert.ok(Date.now() - t0 < 3000)
    assert.ok(!p.stderr().includes(TOKENS.propose), 'token not on stderr')
  } finally { await fake.close() }
})

test('stdio: CHECKGATE_MCP_WRITE=1 registers the proposal tools; SIGTERM exits', async () => {
  const fake = await startFakeApi()
  try {
    const p = start({ CHECKGATE_URL: fake.url, CHECKGATE_TOKEN: TOKENS.propose, CHECKGATE_MCP_WRITE: '1' })
    await initialize(p)
    const names = (await p.send('tools/list', {})).result.tools.map((t) => t.name)
    for (const n of ['create_draft_flag', 'propose_flag_change', 'propose_rollout_change', 'cancel_change_request']) assert.ok(names.includes(n), n)
    const call = await p.send('tools/call', { name: 'propose_rollout_change', arguments: { environment_id: E_OPEN, key: 'checkout', rollout_percentage: 10 } })
    assert.equal(call.result.structuredContent.data.outcome, 'pending_review')
    p.child.kill('SIGTERM')
    const { code, signal } = await within(p.exited, 3000, 'exit after SIGTERM')
    assert.ok(code === 0 || signal === 'SIGTERM', `code=${code} signal=${signal}`)
  } finally { await fake.close() }
})

test('stdio: a call in flight when stdin closes still gets its answer', async () => {
  const fake = await startFakeApi()
  try {
    fake.state.overrides.set(`/api/environments/${E_OPEN}/flags/checkout`, { delayMs: 300, body: { key: 'checkout', is_enabled: true } })
    const p = start({ CHECKGATE_URL: fake.url, CHECKGATE_TOKEN: TOKENS.propose })
    await initialize(p)
    const pending = p.send('tools/call', { name: 'get_flag', arguments: { environment_id: E_OPEN, key: 'checkout' } })
    setTimeout(() => p.child.stdin.end(), 50)
    const res = await pending
    assert.equal(res.result.structuredContent.data.flag.key, 'checkout')
    const { code } = await within(p.exited, 3000, 'exit')
    assert.equal(code, 0)
  } finally { await fake.close() }
})

test('stdio: a URL with credentials is refused at startup (exit 2), without echoing them', async () => {
  const p = start({ CHECKGATE_URL: 'https://ops:hunter2@flags.example.com', CHECKGATE_TOKEN: TOKENS.propose })
  const { code } = await within(p.exited, 30_000, 'exit (startup under load included)')
  assert.equal(code, 2)
  assert.match(p.stderr(), /must not contain credentials/)
  assert.ok(!p.stderr().includes('hunter2'))
  assert.ok(!p.stderr().includes(TOKENS.propose))
})

test('stdio: missing token is a clear startup error', async () => {
  const p = start({ CHECKGATE_URL: 'https://flags.example.com' })
  const { code } = await within(p.exited, 30_000, 'exit (startup under load included)')
  assert.equal(code, 2)
  assert.match(p.stderr(), /CHECKGATE_TOKEN is not set/)
})
