import { describe, test, expect, beforeEach, afterEach, vi } from 'vitest'
import { CheckgateWeb } from '../index.js'

// A faithful in-memory stand-in for the WASM core (snake_case methods, and
// get_variant returns a plain object — matching the real wasm-bindgen binding).
function fakeCore() {
  const flags = new Map()
  return {
    clear_store() { flags.clear() },
    upsert_flag_v2(json) { const f = JSON.parse(json); flags.set(f.key, f) },
    delete_flag(key) { flags.delete(key) },
    is_enabled(key) { const f = flags.get(key); return !!(f && f.is_enabled) },
    get_variant(key) {
      const f = flags.get(key)
      return f ? { enabled: !!f.is_enabled, value: f.default_value ?? null } : null
    },
  }
}

let fetchCalls
beforeEach(() => {
  fetchCalls = []
  vi.stubGlobal('fetch', (url, init) => {
    fetchCalls.push({ url, init })
    return Promise.resolve({ ok: true, status: 204, json: async () => [] })
  })
})
afterEach(() => vi.unstubAllGlobals())

function newClient(opts = {}) {
  return new CheckgateWeb({ serverUrl: 'http://localhost:9999', sdkKey: 'sk_test', ...opts })
}

// Put the client into an evaluatable state without a real connection.
function ready(client, envId = 'env-123') {
  client.core = fakeCore()
  client._ready = true
  client._envId = envId
  return client
}

describe('construction', () => {
  test('applies documented defaults', () => {
    const c = newClient()
    expect(c.reportImpressions).toBe(true)
    expect(c.impressionBatchSize).toBe(50)
    expect(c._envId).toBe(null)
    expect(c._impressions).toEqual([])
    expect(c._events).toEqual([])
    expect(c.isReady()).toBe(false)
  })

  test('parses the bootstrap option (array or {flags} or null)', () => {
    const flags = [{ key: 'a', is_enabled: true }]
    expect(newClient({ bootstrap: flags })._bootstrapFlags).toEqual(flags)
    expect(newClient({ bootstrap: { flags } })._bootstrapFlags).toEqual(flags)
    expect(newClient()._bootstrapFlags).toBe(null)
  })
})

describe('evaluation delegates to the core', () => {
  test('isEnabled / getVariant / getValue', () => {
    const c = ready(newClient({ reportImpressions: false }))
    c.core.upsert_flag_v2(JSON.stringify({ key: 'f', is_enabled: true, flag_type: 'string', default_value: 'blue' }))
    expect(c.isEnabled('f', 'u1')).toBe(true)
    expect(c.getVariant('f', 'u1')).toEqual({ enabled: true, value: 'blue' })
    expect(c.getValue('f', 'u1', {}, 'x')).toBe('blue')
    expect(c.getValue('missing', 'u1', {}, 'fallback')).toBe('fallback')
    expect(c.getVariant('missing', 'u1')).toBe(null)
  })

  test('returns safe defaults before connect', () => {
    const c = newClient({ reportImpressions: false })
    expect(c.isEnabled('f', 'u')).toBe(false)
    expect(c.getVariant('f', 'u')).toBe(null)
  })
})

describe('goal-event tracking', () => {
  test('buffers and flushes to /events with Bearer auth', () => {
    const c = ready(newClient({ impressionBatchSize: 2 }))
    c.track('checkout_complete', 'u1', { value: 49.99, context: { plan: 'pro' } })
    expect(fetchCalls.length).toBe(0) // below batch size
    c.track('checkout_complete', 'u2')
    expect(fetchCalls.length).toBe(1)
    expect(fetchCalls[0].url).toMatch(/\/api\/environments\/env-123\/events$/)
    expect(fetchCalls[0].init.headers.Authorization).toBe('Bearer sk_test')
    const body = JSON.parse(fetchCalls[0].init.body)
    expect(body).toHaveLength(2)
    expect(body[0]).toMatchObject({ event_key: 'checkout_complete', user_id: 'u1', value: 49.99 })
  })

  test('ignores invalid eventKey and pre-connect calls', () => {
    const c = newClient()
    c.track('', 'u')      // invalid key
    c.track('goal', 'u')  // no _envId yet
    expect(fetchCalls.length).toBe(0)
    expect(c._events.length).toBe(0)
  })
})

describe('impressions', () => {
  test('flush posts to /impressions and caps at 500 per batch', () => {
    const c = ready(newClient())
    for (let i = 0; i < 600; i++) c._impressions.push({ flag_key: 'f', user_id: `u${i}`, value: 'true' })
    c._flushImpressions()
    expect(fetchCalls.length).toBe(1)
    expect(fetchCalls[0].url).toMatch(/\/api\/environments\/env-123\/impressions$/)
    expect(JSON.parse(fetchCalls[0].init.body)).toHaveLength(500)
    expect(c._impressions.length).toBe(100)
  })

  test('isEnabled records an impression; reportImpressions:false does not', () => {
    const on = ready(newClient())
    on.core.upsert_flag_v2(JSON.stringify({ key: 'f', is_enabled: true }))
    on.isEnabled('f', 'u1')
    expect(on._impressions.length).toBe(1)

    const off = ready(newClient({ reportImpressions: false }))
    off.core.upsert_flag_v2(JSON.stringify({ key: 'f', is_enabled: true }))
    off.isEnabled('f', 'u1')
    expect(off._impressions.length).toBe(0)
  })
})

test('_formatValue serializes each value type', () => {
  const c = newClient()
  expect(c._formatValue(null)).toBe('null')
  expect(c._formatValue(true)).toBe('true')
  expect(c._formatValue(42)).toBe('42')
  expect(c._formatValue('blue')).toBe('blue')
  expect(c._formatValue({ a: 1 })).toBe('{"a":1}')
})

test('disconnect flushes buffered impressions AND events', () => {
  const c = ready(newClient())
  c._impressions.push({ flag_key: 'f', user_id: 'u', value: 'true' })
  c._events.push({ event_key: 'g', user_id: 'u' })
  c.disconnect()
  const urls = fetchCalls.map((x) => x.url)
  expect(urls.some((u) => /\/impressions$/.test(u))).toBe(true)
  expect(urls.some((u) => /\/events$/.test(u))).toBe(true)
})

// --- Regression coverage for offline namespaces and polling lifecycle -------

test('cache namespaces match SHA-256 without exposing the credential', async () => {
    const { createHash } = await import('node:crypto');
    const { cacheNamespace } = await import('../cache-key.js');
    for (const value of ['', 'abc', 'credential'.repeat(100), 'unicode-\u00e9-\ud83d\ude00']) {
        expect(cacheNamespace(value)).toEqual(createHash('sha256').update(value).digest('hex'));
    }
});

test('persisted flags are isolated between credentials on the same server', async () => {
    const data = new Map();
    const storage = { getItem: k => data.get(k), setItem: (k, v) => data.set(k, v) };
    const prod = newClient({ sdkKey: 'production-secret', storage, reportImpressions: false });
    prod.core = fakeCore();
    prod._applySnapshot([{key:'checkout',is_enabled:true}]);
    const stage = newClient({ sdkKey: 'staging-secret', storage, reportImpressions: false });
    stage.core = fakeCore();
    await stage._hydrateFromCache();
    expect(stage._flagCache.size).toEqual(0);
    expect(prod._cacheKey.includes('production-secret')).toEqual(false);
    prod.disconnect(); stage.disconnect();
});

test('polling bootstrap enables impressions and conversion tracking', async () => {
    const client = newClient();
    client.core = fakeCore();
    vi.stubGlobal('fetch', async () => ({ok:true,headers:{get: () => 'poll-env'},json:async () => [{key:'checkout',is_enabled:true}]}));
    await client._pollSnapshot();
    expect(client.isReady()).toEqual(true);
    expect(client._envId).toEqual('poll-env');
    client.isEnabled('checkout','user');
    client.track('purchase','user');
    expect(client._impressions.length).toEqual(1);
    expect(client._events.length).toEqual(1);
    client.disconnect();
});

test('an outstanding poll cannot overwrite state after SSE takes over', async () => {
    const client = newClient({reportImpressions:false});
    client.core = fakeCore();
    let deliver;
    vi.stubGlobal('fetch', () => new Promise(resolve => {deliver=resolve}));
    const pending = client._pollSnapshot();
    client._stopPollFallback();
    client._applySnapshot([{key:'checkout',is_enabled:false}]);
    deliver({ok:true,json:async () => [{key:'checkout',is_enabled:true}]});
    await pending;
    expect(client._flagCache.get('checkout').is_enabled).toEqual(false);
    client.disconnect();
});

test('overlapping polls share a request and disconnect invalidates its response', async () => {
    const client = newClient({reportImpressions:false});
    client.core = fakeCore();
    let requests = 0, deliver;
    vi.stubGlobal('fetch', () => {requests++; return new Promise(resolve => {deliver=resolve});});
    const first = client._pollSnapshot(), second = client._pollSnapshot();
    expect(requests).toEqual(1);
    client.disconnect();
    deliver({ok:true,json:async () => [{key:'stale',is_enabled:true}]});
    await Promise.all([first,second]);
    expect(client._flagCache.size).toEqual(0);
    expect(client.isReady()).toEqual(false);
});

// --- Reconnect staging, event ordering, and occurrence timestamps ----------

class FakeEventSource {
  constructor(url, opts) { this.url = url; this.opts = opts; this.closed = false; this.listeners = {} }
  addEventListener(name, fn) { this.listeners[name] = fn }
  emit(name, data) { this.listeners[name]({ data: JSON.stringify(data) }) }
  close() { this.closed = true }
}

function connectedClient() {
  vi.stubGlobal('EventSource', FakeEventSource)
  const client = newClient({ reportImpressions: false })
  client.core = fakeCore()
  client._connectDeltas()
  return client
}

function bootstrap(client, flags, envId = 'env-1') {
  client.sse.emit('connected', { environment_id: envId })
  for (const [flag, revision] of flags) {
    client.sse.emit('update', { type: 'UPSERT', env_id: envId, flag, revision })
  }
  client.sse.emit('ready', flags.length)
}

describe('stream resync and ordering', () => {
  test('a reconnect keeps serving the last snapshot until the replay is complete', () => {
    const client = connectedClient()
    bootstrap(client, [[{ key: 'checkout', is_enabled: true }, 1], [{ key: 'search', is_enabled: true }, 2]])
    expect(client.isEnabled('checkout', 'u')).toBe(true)

    // The stream drops and reconnects: during the replay nothing changes.
    client._connectDeltas()
    client.sse.emit('connected', { environment_id: 'env-1' })
    client.sse.emit('update', { type: 'UPSERT', flag: { key: 'checkout', is_enabled: false }, revision: 3 })
    expect(client.isReady()).toBe(true)
    expect(client.isEnabled('checkout', 'u')).toBe(true)
    expect(client.isEnabled('search', 'u')).toBe(true)

    const changed = []
    client.onChange((key) => changed.push(key))
    client.sse.emit('ready', 1)
    expect(client.isEnabled('checkout', 'u')).toBe(false)
    expect(client.isEnabled('search', 'u')).toBe(false)
    expect(changed.sort()).toEqual(['checkout', 'search'])
    client.disconnect()
  })

  test('a replay cut short by a dropped stream is discarded', () => {
    const client = connectedClient()
    bootstrap(client, [[{ key: 'checkout', is_enabled: true }, 1], [{ key: 'search', is_enabled: true }, 2]])
    client.sse.emit('connected', { environment_id: 'env-1' })
    client.sse.emit('update', { type: 'UPSERT', flag: { key: 'checkout', is_enabled: false }, revision: 3 })
    client._scheduleReconnect()
    expect(client.isEnabled('checkout', 'u')).toBe(true)
    expect(client.isEnabled('search', 'u')).toBe(true)
    expect(client._staged).toBe(null)
    client.disconnect()
  })

  test('change events older than the applied revision are ignored', () => {
    const client = connectedClient()
    bootstrap(client, [[{ key: 'checkout', is_enabled: false }, 10]])
    const update = (flag, revision) => client.sse.emit('update', { type: 'UPSERT', flag, revision })

    update({ key: 'checkout', is_enabled: true }, 12)
    update({ key: 'checkout', is_enabled: false }, 11)
    expect(client.isEnabled('checkout', 'u')).toBe(true)

    client.sse.emit('update', { type: 'DELETE', key: 'checkout', revision: 12 })
    update({ key: 'checkout', is_enabled: true }, 12)
    expect(client._flagCache.has('checkout')).toBe(false)
    update({ key: 'checkout', is_enabled: true }, 13)
    expect(client.isEnabled('checkout', 'u')).toBe(true)

    client.sse.emit('update', { type: 'DELETE', key: 'checkout', revision: 12 })
    expect(client.isEnabled('checkout', 'u')).toBe(true)

    update({ key: 'legacy', is_enabled: true })
    update({ key: 'legacy', is_enabled: false })
    expect(client.isEnabled('legacy', 'u')).toBe(false)
    client.disconnect()
  })

  test('impressions and conversions carry the time they happened', () => {
    const client = ready(newClient())
    client.core.upsert_flag_v2(JSON.stringify({ key: 'f', is_enabled: true }))
    const before = Date.now()
    client.isEnabled('f', 'u1')
    client.track('purchase', 'u1')
    for (const stamp of [client._impressions[0].evaluated_at, client._events[0].occurred_at]) {
      const at = Date.parse(stamp)
      expect(at).toBeGreaterThanOrEqual(before)
      expect(at).toBeLessThanOrEqual(Date.now())
    }
  })
})

describe('snapshot consistency', () => {
  test('reconnect listeners run after the whole snapshot is applied', () => {
    const client = connectedClient()
    bootstrap(client, [[{ key: 'a-dependent', is_enabled: false }, 1], [{ key: 'z-base', is_enabled: false }, 2]])
    const seen = {}
    client.onChange((key) => { seen[key] = client.isEnabled('z-base', 'u') })
    bootstrap(client, [[{ key: 'a-dependent', is_enabled: true }, 3], [{ key: 'z-base', is_enabled: true }, 4]])
    expect(seen).toEqual({ 'a-dependent': true, 'z-base': true })
    client.disconnect()
  })

  test('a tombstone in the bootstrap keeps a delayed older event from resurrecting the flag', () => {
    const client = connectedClient()
    client.sse.emit('connected', { environment_id: 'env-1' })
    client.sse.emit('update', { type: 'DELETE', key: 'ghost', revision: 7 })
    client.sse.emit('ready', 0)
    client.sse.emit('update', { type: 'UPSERT', flag: { key: 'ghost', is_enabled: true }, revision: 6 })
    expect(client._flagCache.has('ghost')).toBe(false)
    client.sse.emit('update', { type: 'UPSERT', flag: { key: 'ghost', is_enabled: true }, revision: 8 })
    expect(client.isEnabled('ghost', 'u')).toBe(true)
    client.disconnect()
  })
})

describe('connection lifecycle (v0.1.31 RC2)', () => {
  test('disconnect() during initialisation opens no stream and settles connect()', async () => {
    vi.stubGlobal('EventSource', FakeEventSource)
    const client = newClient({ reportImpressions: false, storage: null })
    const pending = client.connect()
    client.disconnect()
    await expect(pending).rejects.toThrow(/disconnect\(\) was called/)
    await new Promise((r) => setTimeout(r, 0))
    expect(client.sse).toBe(null)
  })

  test('one engine per client: flags survive disconnect() and reconnect', async () => {
    vi.stubGlobal('EventSource', FakeEventSource)
    const client = newClient({ reportImpressions: false, storage: null })
    let pending = client.connect()
    await new Promise((r) => setTimeout(r, 0))
    bootstrap(client, [[{ key: 'checkout', is_enabled: true }, 1]])
    await pending
    const core = client.core
    client.disconnect()
    pending = client.connect()
    await new Promise((r) => setTimeout(r, 0))
    expect(client.core).toBe(core)
    // The replay finds the flag unchanged; it must still evaluate.
    bootstrap(client, [[{ key: 'checkout', is_enabled: true }, 1]])
    await pending
    expect(client.isEnabled('checkout', 'u')).toBe(true)
    client.disconnect()
  })

  test('an unexplained stream failure asks the snapshot endpoint; 401 ends retries', async () => {
    vi.stubGlobal('EventSource', FakeEventSource)
    vi.stubGlobal('fetch', () => Promise.resolve({ ok: false, status: 401 }))
    const client = newClient({ reportImpressions: false, storage: null })
    const pending = client.connect()
    await new Promise((r) => setTimeout(r, 0))
    client.sse.listeners.error({ type: 'error' })
    await expect(pending).rejects.toMatchObject({ name: 'CheckgateAuthError', status: 401 })
    expect(client.getStatus().state).toBe('auth_failed')
    expect(client._reconnectTimer).toBe(null)
    client.disconnect()
  })
})

// --- Monotonic configuration (RC3, R20) -------------------------------------

describe('monotonic configuration (RC3, R20)', () => {
  /** A `?v=2` snapshot response. */
  const revisioned = (flags, revisions, tombstones = {}) =>
    async () => ({ ok: true, headers: { get: () => 'env-1' }, json: async () => ({ v: 2, flags, revisions, tombstones }) })

  test('a delayed replay cannot roll back or resurrect what a newer poll delivered', async () => {
    const client = connectedClient()
    bootstrap(client, [[{ key: 'checkout', is_enabled: true }, 1], [{ key: 'search', is_enabled: true }, 2]])

    // A new stream is opened (its replay is read now), then a poll returns newer state first.
    client._connectDeltas()
    vi.stubGlobal('fetch', revisioned([{ key: 'checkout', is_enabled: false }], { checkout: 5 }, { search: 4 }))
    await client._pollSnapshot()
    expect(client.isEnabled('checkout', 'u')).toBe(false)
    expect(client._flagCache.has('search')).toBe(false)

    // The old replay arrives.
    const changed = []
    client.onChange((key) => changed.push(key))
    client.sse.emit('connected', { environment_id: 'env-1' })
    client.sse.emit('update', { type: 'UPSERT', flag: { key: 'checkout', is_enabled: true }, revision: 1 })
    client.sse.emit('update', { type: 'UPSERT', flag: { key: 'search', is_enabled: true }, revision: 2 })
    client.sse.emit('ready', 2)
    expect(client.isEnabled('checkout', 'u'), 'revision 5 kept over the replay\'s 1').toBe(false)
    expect(client._flagCache.has('search'), 'the deletion at 4 kept over the replay\'s 2').toBe(false)
    expect(changed).toEqual([])

    // The live events behind the replay: older or equal, so nothing happens.
    client.sse.emit('update', { type: 'UPSERT', flag: { key: 'checkout', is_enabled: false }, revision: 5 })
    client.sse.emit('update', { type: 'DELETE', key: 'search', revision: 4 })
    expect(changed).toEqual([])
    client.sse.emit('update', { type: 'UPSERT', flag: { key: 'search', is_enabled: true }, revision: 6 })
    expect(client.isEnabled('search', 'u')).toBe(true)
    expect(changed).toEqual(['search'])
    client.disconnect()
  })

  test('a newer replay still wins over an older poll; a later set removes what it lacks', async () => {
    const client = connectedClient()
    bootstrap(client, [[{ key: 'checkout', is_enabled: true }, 1], [{ key: 'legacy', is_enabled: true }, 2]])
    client._connectDeltas()
    vi.stubGlobal('fetch', revisioned([{ key: 'checkout', is_enabled: false }, { key: 'legacy', is_enabled: true }], { checkout: 3, legacy: 2 }))
    await client._pollSnapshot()
    client.sse.emit('connected', { environment_id: 'env-1' })
    client.sse.emit('update', { type: 'UPSERT', flag: { key: 'checkout', is_enabled: true }, revision: 7 })
    client.sse.emit('ready', 1)
    expect(client.isEnabled('checkout', 'u')).toBe(true)
    // No entry at all for `legacy` in the replay, learned by a poll after the
    // stream request: created after the replay was read, so kept.
    expect(client._flagCache.has('legacy')).toBe(true)

    client._connectDeltas()
    client.sse.emit('connected', { environment_id: 'env-1' })
    client.sse.emit('update', { type: 'UPSERT', flag: { key: 'checkout', is_enabled: true }, revision: 7 })
    client.sse.emit('ready', 1)
    expect(client._flagCache.has('legacy')).toBe(false)
    client.disconnect()
  })

  test('a duplicate event changes nothing and notifies nobody', () => {
    const client = connectedClient()
    bootstrap(client, [[{ key: 'checkout', is_enabled: true }, 3]])
    const changed = []
    client.onChange((key) => changed.push(key))
    client.sse.emit('update', { type: 'UPSERT', flag: { key: 'checkout', is_enabled: true }, revision: 3 })
    client.sse.emit('update', { type: 'DELETE', key: 'never-held', revision: 9 })
    expect(changed).toEqual([])
    client.sse.emit('update', { type: 'DELETE', key: 'checkout', revision: 3 })
    client.sse.emit('update', { type: 'DELETE', key: 'checkout', revision: 3 })
    expect(changed).toEqual(['checkout'])
    client.disconnect()
  })

  test('an older event staged during a replay cannot replace a newer staged one', () => {
    const client = connectedClient()
    client.sse.emit('connected', { environment_id: 'env-1' })
    client.sse.emit('update', { type: 'UPSERT', flag: { key: 'checkout', is_enabled: true }, revision: 8 })
    client.sse.emit('update', { type: 'UPSERT', flag: { key: 'checkout', is_enabled: false }, revision: 7 })
    client.sse.emit('ready', 1)
    expect(client.isEnabled('checkout', 'u')).toBe(true)
    client.disconnect()
  })

  test('a malformed revisioned snapshot leaves the flags as they were', async () => {
    for (const body of [
      { v: 2, flags: [{ key: 'checkout', is_enabled: false }], revisions: {} },
      { v: 2, flags: [{ key: 'checkout', is_enabled: false }], revisions: { checkout: '5' } },
      { v: 2, flags: [{ key: 'checkout', is_enabled: false }], revisions: { checkout: 5 }, tombstones: { x: null } },
      { v: 2, flags: 'nope' },
      { v: 2, flags: [{ key: 'constructor', is_enabled: false }], revisions: {} },
      'nope',
    ]) {
      const client = ((c) => { c.core = fakeCore(); return c })(newClient({ reportImpressions: false }))
      vi.stubGlobal('fetch', revisioned([{ key: 'checkout', is_enabled: true }], { checkout: 1 }))
      await client._pollSnapshot()
      vi.stubGlobal('fetch', async () => ({ ok: true, headers: { get: () => null }, json: async () => body }))
      await client._pollSnapshot()
      expect(client.isEnabled('checkout', 'u'), JSON.stringify(body)).toBe(true)
      expect(client._revisions.get('checkout').revision).toBe(1)
      client.disconnect()
    }
  })

  test('polls ask for ?v=2; a server without revisions gets the old behaviour', async () => {
    const client = ((c) => { c.core = fakeCore(); return c })(newClient({ reportImpressions: false }))
    const urls = []
    vi.stubGlobal('fetch', async (url) => { urls.push(url); return { ok: true, headers: { get: () => null }, json: async () => [{ key: 'checkout', is_enabled: true }] } })
    await client._pollSnapshot()
    expect(urls[0].endsWith('/flags/snapshot?v=2')).toBe(true)
    expect(client.isEnabled('checkout', 'u')).toBe(true)
    expect(client._revisions.size, 'no revisions invented (never revision 0)').toBe(0)
    client.disconnect()
  })
})
