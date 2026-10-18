import { describe, test, expect, beforeEach, afterEach, vi } from 'vitest'

// The RN SDK reads its native evaluation engine from the JSI global
// `global.__CheckgateInternal` (installed by installCheckgateJSI() on-device).
// We install a faithful fake there BEFORE importing the SDK. Bridge methods are
// camelCase and getVariant returns a JSON string (matching the JSI host).
function fakeBridge() {
  const flags = new Map()
  return {
    clearStore() { flags.clear() },
    upsertFlagV2(flag) { flags.set(flag.key, flag) },
    deleteFlag(key) { flags.delete(key) },
    isEnabled(key) { const f = flags.get(key); return !!(f && f.is_enabled) },
    getVariant(key) {
      const f = flags.get(key)
      if (!f) return 'null'
      return JSON.stringify({ enabled: !!f.is_enabled, value: f.default_value ?? null })
    },
  }
}

globalThis.__CheckgateInternal = fakeBridge()
const { CheckgateNativeClient } = await import('../index.js')

let fetchCalls
beforeEach(() => {
  fetchCalls = []
  globalThis.__CheckgateInternal = fakeBridge()
  vi.stubGlobal('fetch', (url, init) => {
    fetchCalls.push({ url, init })
    return Promise.resolve({ ok: true, status: 204, json: async () => [] })
  })
})
afterEach(() => vi.unstubAllGlobals())

function newClient(opts = {}) {
  return new CheckgateNativeClient({ serverUrl: 'http://localhost:9999', sdkKey: 'sk_test', ...opts })
}

function ready(client, envId = 'env-123') {
  client._ready = true
  client._hydrated = true
  client._envId = envId
  return client
}

test('constructor binds the JSI bridge and applies defaults', () => {
  const c = newClient()
  expect(c.bridge).toBeDefined()
  expect(c.reportImpressions).toBe(true)
  expect(c.impressionBatchSize).toBe(50)
  expect(c._impressions).toEqual([])
  expect(c._events).toEqual([])
})

describe('evaluation delegates to the JSI bridge', () => {
  test('isEnabled / getVariant / getValue', () => {
    const c = ready(newClient({ reportImpressions: false }))
    c.bridge.upsertFlagV2({ key: 'f', is_enabled: true, flag_type: 'string', default_value: 'blue' })
    expect(c.isEnabled('f', 'u1')).toBe(true)
    expect(c.getVariant('f', 'u1')).toEqual({ enabled: true, value: 'blue' })
    expect(c.getValue('f', 'u1', {}, 'x')).toBe('blue')
    expect(c.getValue('missing', 'u1', {}, 'fallback')).toBe('fallback')
    expect(c.getVariant('missing', 'u1')).toBe(null)
  })
})

describe('goal-event tracking', () => {
  test('buffers and flushes to /events with Bearer auth', () => {
    const c = ready(newClient({ impressionBatchSize: 2 }))
    c.track('checkout_complete', 'u1', { value: 49.99 })
    expect(fetchCalls.length).toBe(0)
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
    c.track('', 'u')
    c.track('goal', 'u')
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

    prod._applySnapshot([{key:'checkout',is_enabled:true}]);
    const stage = newClient({ sdkKey: 'staging-secret', storage, reportImpressions: false });
    stage.bridge = fakeBridge();
    await stage._hydrateFromCache();
    expect(stage._flagCache.size).toEqual(0);
    expect(prod._cacheKey.includes('production-secret')).toEqual(false);
    prod.disconnect(); stage.disconnect();
});

test('polling bootstrap enables impressions and conversion tracking', async () => {
    const client = newClient();

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
    client.bridge.upsertFlagV2({ key: 'f', is_enabled: true })
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

describe('native module installation', () => {
  test('asks the native module to install the JSI host when it is missing', async () => {
    const { NativeModules } = await import('react-native')
    const installed = fakeBridge()
    let calls = 0
    delete globalThis.__CheckgateInternal
    NativeModules.Checkgate = {
      install() {
        calls++
        globalThis.__CheckgateInternal = installed
        return true
      },
    }
    try {
      expect(newClient().bridge).toBe(installed)
      expect(newClient().bridge).toBe(installed)
      expect(calls).toBe(1)
    } finally {
      delete NativeModules.Checkgate
    }
  })

  test('connect() explains a missing native module', async () => {
    delete globalThis.__CheckgateInternal
    await expect(newClient().connect()).rejects.toThrow(/Native module not found/)
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
