import { test } from 'node:test'
import assert from 'node:assert/strict'
import { connect, dataBlock } from './helpers.js'
import { E_OPEN, E_PROTECTED, INJECTION, P1, TOKENS } from './fake-api.js'

const READ_TOOLS = [
  'get_access_info', 'list_projects', 'list_environments', 'list_flags', 'get_flag', 'explain_flag',
  'flag_dependencies', 'flag_audit_events', 'compare_flag_across_environments', 'list_change_requests',
  'sdk_connection_summary',
]
const WRITE_TOOLS = ['create_draft_flag', 'propose_flag_change', 'propose_rollout_change', 'cancel_change_request']
const FORBIDDEN_TOOLS = /approve|reject|delete|promote|archive|http|request|raw/i

function writes(fake) {
  return fake.state.requests.filter((r) => r.method !== 'GET')
}

test('read-only by default: write tools are not registered at all', async () => {
  const s = await connect()
  try {
    const { tools } = await s.client.listTools()
    assert.deepEqual(tools.map((t) => t.name).sort(), [...READ_TOOLS].sort())
    for (const t of tools) {
      assert.equal(t.annotations?.readOnlyHint, true, t.name)
      assert.equal(t.inputSchema.additionalProperties, false, `${t.name} rejects unknown arguments`)
    }
    const r = await s.call('propose_flag_change', { environment_id: E_OPEN, key: 'checkout', changes: { is_enabled: false } })
    assert.equal(r.isError, true)
    assert.equal(writes(s.fake).length, 0)
  } finally { await s.close() }
})

test('CHECKGATE_MCP_WRITE=1 adds exactly the four proposal tools — no approve/reject/delete/raw HTTP', async () => {
  const s = await connect({ write: true })
  try {
    const names = (await s.client.listTools()).tools.map((t) => t.name)
    assert.deepEqual(names.sort(), [...READ_TOOLS, ...WRITE_TOOLS].sort())
    for (const n of names) {
      if (n === 'cancel_change_request' || n === 'list_change_requests') continue
      assert.doesNotMatch(n, FORBIDDEN_TOOLS, n)
    }
  } finally { await s.close() }
})

test('every read tool returns delimited data and structuredContent', async () => {
  const s = await connect()
  try {
    const cases = [
      ['get_access_info', {}],
      ['list_projects', {}],
      ['list_environments', { project_id: P1 }],
      ['list_flags', { environment_id: E_OPEN }],
      ['get_flag', { environment_id: E_OPEN, key: 'checkout' }],
      ['explain_flag', { environment_id: E_OPEN, key: 'checkout' }],
      ['flag_dependencies', { environment_id: E_OPEN, key: 'base' }],
      ['flag_audit_events', { environment_id: E_OPEN, key: 'checkout' }],
      ['compare_flag_across_environments', { key: 'checkout', environment_a: E_OPEN, environment_b: E_PROTECTED }],
      ['list_change_requests', { environment_id: E_OPEN }],
      ['sdk_connection_summary', {}],
    ]
    assert.deepEqual(cases.map((c) => c[0]).sort(), [...READ_TOOLS].sort())
    for (const [name, args] of cases) {
      const r = await s.call(name, args)
      assert.notEqual(r.isError, true, `${name}: ${r.content?.[0]?.text}`)
      assert.deepEqual(dataBlock(r), r.structuredContent.data, name)
    }
    assert.equal(writes(s.fake).length, 0, 'read tools never write')
  } finally { await s.close() }
})

test('read tool results carry the right content', async () => {
  const s = await connect()
  try {
    const access = (await s.call('get_access_info')).structuredContent.data
    assert.equal(access.token_scope, 'propose')
    assert.equal(access.write_tools_enabled, false)

    const envs = (await s.call('list_environments', { project_id: P1 })).structuredContent.data.environments
    assert.deepEqual(envs.map((e) => [e.id, e.require_approval]), [[E_OPEN, false], [E_PROTECTED, true]])

    const page = (await s.call('list_flags', { environment_id: E_OPEN, limit: 2 })).structuredContent.data
    assert.deepEqual(page.flags.map((f) => f.key), ['base', 'checkout'])
    assert.equal(page.next_offset, 2)
    const rest = (await s.call('list_flags', { environment_id: E_OPEN, limit: 2, offset: 2 })).structuredContent.data
    assert.deepEqual(rest.flags.map((f) => f.key), ['needs-base'])
    assert.equal(rest.next_offset, null)
    const q = s.fake.state.requests.find((r) => r.path === `/api/environments/${E_OPEN}/flags` && r.query.offset === '2')
    assert.equal(q.query.limit, '2')

    const steps = (await s.call('explain_flag', { environment_id: E_OPEN, key: 'checkout' })).structuredContent.data.steps
    assert.match(steps[0], /ENABLED/)
    assert.ok(steps.some((x) => x.includes('"country" equals one of ["PH", "SG"]')), steps.join('\n'))
    assert.ok(steps.some((x) => x.includes('segment "beta"') && x.includes('"@acme.test"')), 'segment rule expanded')
    assert.ok(steps.some((x) => x.includes('Rollout 30 %')))

    const deps = (await s.call('flag_dependencies', { environment_id: E_OPEN, key: 'needs-base' })).structuredContent.data
    assert.deepEqual(deps.prerequisites.map((p) => [p.flag_key, p.exists]), [['base', true], ['ghost', false]])
    const base = (await s.call('flag_dependencies', { environment_id: E_OPEN, key: 'base' })).structuredContent.data
    assert.deepEqual(base.dependents.map((d) => d.flag_key), ['needs-base'])
    const missing = await s.call('flag_dependencies', { environment_id: E_OPEN, key: 'nope' })
    assert.equal(missing.isError, true)

    const audit = (await s.call('flag_audit_events', { environment_id: E_OPEN, key: 'checkout' })).structuredContent.data.events
    assert.deepEqual(audit[0].changes.map((c) => c.field).sort(), ['description', 'is_enabled'])
    assert.ok(!('before_data' in audit[0]), 'only the changed fields')

    const diff = (await s.call('compare_flag_across_environments', { key: 'checkout', environment_a: E_OPEN, environment_b: E_PROTECTED })).structuredContent.data
    assert.deepEqual(diff.evaluation_differences.map((d) => d.field).sort(), ['is_enabled', 'rollout_percentage', 'rules'])
    assert.deepEqual(diff.metadata_differences.map((d) => d.field).sort(), ['description', 'owner_email'])
    const onlyA = await s.call('compare_flag_across_environments', { key: 'base', environment_a: E_OPEN, environment_b: E_PROTECTED })
    assert.match(onlyA.content[0].text, /exists only in environment A/)

    const conn = (await s.call('sdk_connection_summary')).structuredContent.data
    assert.deepEqual(conn.per_environment, [{ environment_id: E_OPEN, connections: 2 }, { environment_id: E_PROTECTED, connections: 1 }])
    assert.equal(conn.without_environment, 1)
    const text = JSON.stringify(conn)
    for (const leak of ['203.0.113', '198.51.100', '192.0.2', 'c1', 'web']) assert.ok(!text.includes(leak), `connection summary leaks ${leak}`)
  } finally { await s.close() }
})

test('prompt injection in user content is returned verbatim, as data, and triggers nothing', async () => {
  const s = await connect({ write: true })
  try {
    const before = s.fake.state.requests.length
    const r = await s.call('get_flag', { environment_id: E_OPEN, key: 'checkout' })
    const text = r.content[0].text
    // Verbatim inside the data field...
    assert.equal(r.structuredContent.data.flag.description, INJECTION)
    assert.equal(dataBlock(r).flag.description, INJECTION)
    // ...the block cannot be closed early by the content...
    assert.equal(text.split('</checkgate-data>').length, 2, 'exactly one closing delimiter')
    assert.ok(!text.slice(0, text.indexOf('<checkgate-data>')).includes('Ignore previous'), 'nothing before the block')
    // ...and the server made exactly the one GET, nothing else.
    const made = s.fake.state.requests.slice(before)
    assert.deepEqual(made.map((x) => `${x.method} ${x.path}`), [`GET /api/environments/${E_OPEN}/flags/checkout`])
    // Same for audit text and project names.
    const audit = await s.call('flag_audit_events', { environment_id: E_OPEN })
    assert.equal(audit.structuredContent.data.events[0].changes.find((c) => c.field === 'description').after, INJECTION)
    assert.equal(writes(s.fake).length, 0)
    assert.equal(s.fake.state.changeRequests[E_OPEN].length, 0)
  } finally { await s.close() }
})

test('propose token: PATCH is sent and comes back as a pending change request', async () => {
  const s = await connect({ write: true })
  try {
    const r = await s.call('propose_flag_change', { environment_id: E_OPEN, key: 'checkout', changes: { is_enabled: false, rules: [] } })
    assert.notEqual(r.isError, true, r.content[0].text)
    assert.equal(r.structuredContent.data.outcome, 'pending_review')
    assert.match(r.content[0].text, /PENDING review/)
    assert.equal(s.fake.state.flags[E_OPEN].get('checkout').is_enabled, true, 'nothing applied')
    const patch = writes(s.fake).find((x) => x.method === 'PATCH')
    assert.deepEqual(patch.body, { is_enabled: false, rules: [] })

    const roll = await s.call('propose_rollout_change', { environment_id: E_PROTECTED, key: 'checkout', rollout_percentage: 50 })
    assert.equal(roll.structuredContent.data.outcome, 'pending_review')
    assert.deepEqual(writes(s.fake).at(-1).body, { rollout_percentage: 50 })

    const crs = (await s.call('list_change_requests', { environment_id: E_OPEN, status: 'pending' })).structuredContent.data
    assert.equal(crs.total, 1)
    const id = crs.change_requests[0].id
    const c = await s.call('cancel_change_request', { environment_id: E_OPEN, change_request_id: id })
    assert.equal(c.structuredContent.data.outcome, 'withdrawn')
    assert.equal(writes(s.fake).at(-1).method, 'DELETE')
    const again = await s.call('cancel_change_request', { environment_id: E_OPEN, change_request_id: id })
    assert.equal(again.isError, true)
    assert.equal(again.structuredContent.error.kind, 'not_found')
  } finally { await s.close() }
})

test('propose guard: a read_write token never sends a PATCH to an unprotected environment', async () => {
  const s = await connect({ scope: 'read_write', write: true })
  try {
    for (const [name, args] of [
      ['propose_flag_change', { environment_id: E_OPEN, key: 'checkout', changes: { is_enabled: false } }],
      ['propose_rollout_change', { environment_id: E_OPEN, key: 'checkout', rollout_percentage: 0 }],
      ['create_draft_flag', { environment_id: E_OPEN, key: 'draft-x' }],
      ['cancel_change_request', { environment_id: E_OPEN, change_request_id: 1 }],
    ]) {
      const r = await s.call(name, args)
      assert.equal(r.isError, true, name)
      assert.equal(r.structuredContent.error.kind, 'refused', name)
    }
    assert.equal(writes(s.fake).length, 0, 'nothing was sent')
    assert.equal(s.fake.state.flags[E_OPEN].get('checkout').is_enabled, true)
    // Where approval is required, the server queues any PATCH: allowed.
    const r = await s.call('propose_flag_change', { environment_id: E_PROTECTED, key: 'checkout', changes: { is_enabled: true } })
    assert.equal(r.structuredContent.data.outcome, 'pending_review')
  } finally { await s.close() }
})

test('guard off + read_write on an unprotected environment: the direct apply is reported loudly', async () => {
  const s = await connect({ scope: 'read_write', write: true, env: { CHECKGATE_MCP_REQUIRE_PROPOSE: '0' } })
  try {
    const r = await s.call('propose_flag_change', { environment_id: E_OPEN, key: 'checkout', changes: { is_enabled: false } })
    assert.notEqual(r.isError, true)
    assert.equal(r.structuredContent.data.outcome, 'applied_immediately')
    assert.match(r.content[0].text, /^WARNING: Checkgate APPLIED THIS CHANGE IMMEDIATELY \(HTTP 200\)/)
    // create_draft_flag still never overwrites, even where the server would.
    const c = await s.call('create_draft_flag', { environment_id: E_OPEN, key: 'checkout' })
    assert.equal(c.structuredContent.error.kind, 'conflict')
    assert.equal(writes(s.fake).filter((x) => x.method === 'POST').length, 0)
  } finally { await s.close() }
})

test('unknown scope (ambiguous or not a PAT) counts as not-propose', async () => {
  // /api/tokens refused (e.g. an SDK key, or a server error): scope "unknown".
  const s = await connect({ scope: 'read_write', write: true })
  try {
    s.fake.state.overrides.set('/api/tokens', { status: 403 })
    const r = await s.call('get_access_info')
    assert.equal(r.structuredContent.data.token_scope, 'unknown')
  } finally { await s.close() }
})

test('create_draft_flag always sends a disabled flag', async () => {
  const s = await connect({ write: true })
  try {
    const r = await s.call('create_draft_flag', {
      environment_id: E_OPEN, key: 'agent-draft', description: 'new', flag_type: 'string', default_value: 'v2', tags: ['agent'],
    })
    assert.notEqual(r.isError, true, r.content[0].text)
    assert.equal(r.structuredContent.data.outcome, 'created_disabled')
    const post = writes(s.fake).find((x) => x.method === 'POST')
    assert.equal(post.body.is_enabled, false)
    assert.equal(post.body.rollout_percentage, null)
    assert.ok(!('disabled_value' in post.body))
    // The schema has no way to ask for an enabled flag.
    const bad = await s.call('create_draft_flag', { environment_id: E_OPEN, key: 'x2', is_enabled: true })
    assert.equal(bad.isError, true)
    // Existing key → 409 from the server, surfaced as conflict.
    const dup = await s.call('create_draft_flag', { environment_id: E_OPEN, key: 'checkout' })
    assert.equal(dup.structuredContent.error.kind, 'conflict')
  } finally { await s.close() }
})

test('schema bounds are enforced before anything is sent', async () => {
  const s = await connect({ write: true })
  try {
    const bad = [
      ['get_flag', { environment_id: 'not-a-uuid', key: 'checkout' }],
      ['get_flag', { environment_id: E_OPEN, key: 'has space' }],
      ['get_flag', { environment_id: E_OPEN, key: 'k'.repeat(101) }],
      ['get_flag', { environment_id: E_OPEN, key: '../../tokens' }],
      ['get_flag', { environment_id: E_OPEN, key: 'checkout', extra: 1 }],
      ['list_flags', { environment_id: E_OPEN, limit: 201 }],
      ['list_flags', { environment_id: E_OPEN, limit: 0 }],
      ['list_flags', { environment_id: E_OPEN, offset: -1 }],
      ['list_flags', { environment_id: E_OPEN, limit: 1.5 }],
      ['flag_audit_events', { environment_id: E_OPEN, limit: 101 }],
      ['list_change_requests', { environment_id: E_OPEN, status: 'merged' }],
      ['propose_rollout_change', { environment_id: E_OPEN, key: 'checkout', rollout_percentage: 101 }],
      ['propose_rollout_change', { environment_id: E_OPEN, key: 'checkout', rollout_percentage: -1 }],
      ['propose_flag_change', { environment_id: E_OPEN, key: 'checkout', changes: {} }],
      ['propose_flag_change', { environment_id: E_OPEN, key: 'checkout', changes: { key: 'other' } }],
      ['propose_flag_change', { environment_id: E_OPEN, key: 'checkout', changes: { archived_at: null } }],
      ['propose_flag_change', { environment_id: E_OPEN, key: 'checkout', changes: { rules: [{ operator: 'regex', attribute: 'a', values: [] }] } }],
      ['propose_flag_change', { environment_id: E_OPEN, key: 'checkout', changes: { rules: [{ operator: 'equals', values: ['x'] }] } }],
      ['propose_flag_change', { environment_id: E_OPEN, key: 'checkout', changes: { rules: Array.from({ length: 101 }, () => ({ attribute: 'a' })) } }],
      ['propose_flag_change', { environment_id: E_OPEN, key: 'checkout', changes: { default_value: 'x'.repeat(20_000) } }],
      ['propose_flag_change', { environment_id: E_OPEN, key: 'checkout', changes: { description: 'd'.repeat(1001) } }],
      ['propose_flag_change', { environment_id: E_OPEN, key: 'checkout', changes: { tags: Array.from({ length: 21 }, (_, i) => `t${i}`) } }],
      ['cancel_change_request', { environment_id: E_OPEN, change_request_id: 0 }],
      ['create_draft_flag', { environment_id: E_OPEN, key: 'x', flag_type: 'float' }],
      ['create_draft_flag', { environment_id: E_OPEN, key: 'x', owner_email: 'not an email' }],
    ]
    const before = s.fake.state.requests.length
    for (const [name, args] of bad) {
      const r = await s.call(name, args)
      assert.equal(r.isError, true, `${name} ${JSON.stringify(args).slice(0, 120)}`)
      assert.match(r.content[0].text, /validation/i, name)
    }
    assert.equal(s.fake.state.requests.length, before, 'no request was sent for invalid input')
  } finally { await s.close() }
})

test('HTTP errors map to clear, stable tool errors', async () => {
  const s = await connect()
  try {
    const path = `/api/environments/${E_OPEN}/flags/checkout`
    const cases = [
      [401, 'auth', /HTTP 401/],
      [403, 'forbidden', /HTTP 403.*propose/],
      [404, 'not_found', /HTTP 404/],
      [409, 'conflict', /HTTP 409/],
      [422, 'invalid', /HTTP 422/],
      [429, 'rate_limited', /retry after 7 s/],
      [500, 'server', /HTTP 500/],
      [418, 'http', /HTTP 418/],
    ]
    for (const [status, kind, msg] of cases) {
      s.fake.state.overrides.set(path, { status, headers: status === 429 ? { 'retry-after': '7' } : {}, body: { error: `body text ${TOKENS.propose}` } })
      const r = await s.call('get_flag', { environment_id: E_OPEN, key: 'checkout' })
      assert.equal(r.isError, true)
      assert.equal(r.structuredContent.error.kind, kind, String(status))
      assert.equal(r.structuredContent.error.status, status)
      assert.match(r.content[0].text, msg)
      assert.ok(!r.content[0].text.includes('body text'), 'response bodies are not echoed')
    }
    s.fake.state.overrides.set(path, { raw: '<html>login</html>', headers: { 'content-type': 'text/html' } })
    const html = await s.call('get_flag', { environment_id: E_OPEN, key: 'checkout' })
    assert.equal(html.structuredContent.error.kind, 'bad_response')
  } finally { await s.close() }
})

test('timeouts and oversized responses fail fast and cleanly', async () => {
  const s = await connect({ env: { CHECKGATE_TIMEOUT_MS: '200', CHECKGATE_MCP_MAX_RESPONSE_BYTES: '4096' } })
  try {
    const path = `/api/environments/${E_OPEN}/flags/checkout`
    s.fake.state.overrides.set(path, { delayMs: 2000, body: {} })
    const t0 = Date.now()
    const r = await s.call('get_flag', { environment_id: E_OPEN, key: 'checkout' })
    assert.equal(r.structuredContent.error.kind, 'timeout')
    assert.ok(Date.now() - t0 < 1500, `took ${Date.now() - t0} ms`)

    s.fake.state.overrides.set(path, { body: { description: 'x'.repeat(10_000) } })
    const big = await s.call('get_flag', { environment_id: E_OPEN, key: 'checkout' })
    assert.equal(big.structuredContent.error.kind, 'too_large')
  } finally { await s.close() }
})

test('the token never appears in any result, error or log', async () => {
  const s = await connect({ write: true })
  try {
    const outputs = []
    for (const [name, args] of [
      ['get_access_info', {}],
      ['get_flag', { environment_id: E_OPEN, key: 'checkout' }],
      ['get_flag', { environment_id: E_OPEN, key: 'missing' }],
      ['propose_flag_change', { environment_id: E_OPEN, key: 'checkout', changes: { is_enabled: false } }],
    ]) outputs.push(JSON.stringify(await s.call(name, args)))
    s.fake.state.overrides.set('/api/projects', { status: 500 })
    outputs.push(JSON.stringify(await s.call('list_projects')))
    const all = outputs.join('\n') + s.logs.join('\n')
    assert.ok(!all.includes(TOKENS.propose))
    assert.ok(!all.includes(TOKENS.propose.slice(4, 20)), 'not even a fragment past the public prefix')
    // ...and it is only ever sent as a Bearer header, never in a URL.
    for (const req of s.fake.state.requests) {
      assert.equal(req.headers.authorization, `Bearer ${TOKENS.propose}`)
      assert.ok(!req.path.includes(TOKENS.propose) && !JSON.stringify(req.query).includes(TOKENS.propose))
    }
  } finally { await s.close() }
})
