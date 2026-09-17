'use strict'

// Unit tests for the Node.js SDK client logic — evaluation delegation, goal-event
// tracking, impression/event batching, and shutdown flushing.
//
// The SDK's constructor instantiates the native NAPI core and its module `require`s
// `./native-binding.js` (a build artifact) and `eventsource`. We mock both via a
// Module._load shim BEFORE requiring the SDK, so these tests run without a native
// build and without a real SSE connection. The fake core is a faithful in-memory
// stand-in implementing exactly the methods the SDK calls.

const { test, beforeEach, afterEach } = require('node:test')
const assert = require('node:assert')
const Module = require('node:module')

function makeFakeCore() {
  const flags = new Map()
  return {
    flags,
    clearStore() { flags.clear() },
    upsertFlagV2(json) { const f = JSON.parse(json); flags.set(f.key, f) },
    deleteFlag(key) { flags.delete(key) },
    isEnabled(key) { const f = flags.get(key); return !!(f && f.is_enabled) },
    // The real NAPI core returns a JSON string; the SDK JSON.parses it.
    getVariant(key) {
      const f = flags.get(key)
      if (!f) return 'null'
      return JSON.stringify({ enabled: !!f.is_enabled, value: f.default_value ?? null })
    },
  }
}

class FakeEventSource {
  constructor(url, opts) { this.url = url; this.opts = opts; this.closed = false; this.listeners = {} }
  addEventListener(name, fn) { this.listeners[name] = fn }
  emit(name, data) { this.listeners[name]({ data: JSON.stringify(data) }) }
  close() { this.closed = true }
}

// Intercept the two module requires the SDK makes at load time.
const origLoad = Module._load
Module._load = function (request) {
  if (request === './native-binding.js') {
    return { CheckgateCore: function () { return makeFakeCore() } }
  }
  if (request === 'eventsource') {
    return { EventSource: FakeEventSource }
  }
  return origLoad.apply(this, arguments)
}
const { CheckgateClient } = require('../index.js')
Module._load = origLoad // restore; the SDK already captured the mocks at load

// --- fetch recorder --------------------------------------------------------

let fetchCalls
const realFetch = global.fetch
beforeEach(() => {
  fetchCalls = []
  global.fetch = (url, init) => {
    fetchCalls.push({ url, init })
    return Promise.resolve({ ok: true, status: 204, json: async () => [] })
  }
})
afterEach(() => { global.fetch = realFetch })

function newClient(opts = {}) {
  return new CheckgateClient({ serverUrl: 'http://localhost:9999', sdkKey: 'sk_test', ...opts })
}

// --- Construction ----------------------------------------------------------

test('constructor applies documented defaults', () => {
  const c = newClient()
  assert.equal(c.reportImpressions, true)
  assert.equal(c.impressionBatchSize, 50)
  assert.equal(c.impressionFlushIntervalMs, 10000)
  assert.equal(c._envId, null)
  assert.deepEqual(c._impressions, [])
  assert.deepEqual(c._events, [])
  assert.equal(c.isReady(), false)
  assert.ok(c.core, 'native core constructed')
  c.disconnect()
})

// --- Evaluation delegation -------------------------------------------------

test('isEnabled / getVariant / getValue delegate to the core once ready', () => {
  const c = newClient({ reportImpressions: false })
  c._hydrated = true // simulate a completed bootstrap without a real connection
  c.core.upsertFlagV2(JSON.stringify({ key: 'f', is_enabled: true, flag_type: 'string', default_value: 'blue' }))

  assert.equal(c.isEnabled('f', 'u1'), true)
  assert.deepEqual(c.getVariant('f', 'u1'), { enabled: true, value: 'blue' })
  assert.equal(c.getValue('f', 'u1', {}, 'x'), 'blue')
  assert.equal(c.getValue('missing', 'u1', {}, 'fallback'), 'fallback')
  assert.equal(c.getVariant('missing', 'u1'), null)
  c.disconnect()
})

test('evaluation before connect returns safe defaults', () => {
  const c = newClient({ reportImpressions: false })
  // _ready and _hydrated both false
  assert.equal(c.isEnabled('f', 'u'), false)
  assert.equal(c.getVariant('f', 'u'), null)
  c.disconnect()
})

// --- Goal-event tracking ---------------------------------------------------

test('track() buffers and flushes a batch to /events with Bearer auth', () => {
  const c = newClient({ impressionBatchSize: 2 })
  c._envId = 'env-123'

  c.track('checkout_complete', 'u1', { value: 49.99, context: { plan: 'pro' } })
  assert.equal(fetchCalls.length, 0, 'below batch size — not flushed yet')

  c.track('checkout_complete', 'u2') // reaches batch size → flush
  assert.equal(fetchCalls.length, 1)
  assert.match(fetchCalls[0].url, /\/api\/environments\/env-123\/events$/)
  assert.equal(fetchCalls[0].init.headers.Authorization, 'Bearer sk_test')

  const body = JSON.parse(fetchCalls[0].init.body)
  assert.equal(body.length, 2)
  assert.equal(body[0].event_key, 'checkout_complete')
  assert.equal(body[0].user_id, 'u1')
  assert.equal(body[0].value, 49.99)
  assert.deepEqual(body[0].context, { plan: 'pro' })
  c.disconnect()
})

test('track() ignores an invalid eventKey and pre-connect calls', () => {
  const c = newClient()
  c.track('', 'u')            // invalid key
  c.track('goal', 'u')        // no _envId resolved yet → dropped
  assert.equal(fetchCalls.length, 0)
  assert.equal(c._events.length, 0)
  c.disconnect()
})

// --- Impression batching ---------------------------------------------------

test('_flushImpressions posts to /impressions and caps at 500 per batch', () => {
  const c = newClient()
  c._envId = 'env-123'
  for (let i = 0; i < 600; i++) {
    c._impressions.push({ flag_key: 'f', user_id: `u${i}`, value: 'true' })
  }
  c._flushImpressions()
  assert.equal(fetchCalls.length, 1)
  assert.match(fetchCalls[0].url, /\/api\/environments\/env-123\/impressions$/)
  assert.equal(JSON.parse(fetchCalls[0].init.body).length, 500)
  assert.equal(c._impressions.length, 100, 'remainder kept for the next flush')
  c.disconnect()
})

test('isEnabled records an impression when reporting is on', () => {
  const c = newClient()
  c._hydrated = true
  c._envId = 'env-123'
  c.core.upsertFlagV2(JSON.stringify({ key: 'f', is_enabled: true }))
  c.isEnabled('f', 'u1')
  assert.equal(c._impressions.length, 1)
  assert.equal(c._impressions[0].flag_key, 'f')
  assert.equal(c._impressions[0].value, 'true')
  c.disconnect()
})

test('reportImpressions:false records nothing', () => {
  const c = newClient({ reportImpressions: false })
  c._hydrated = true
  c._envId = 'env-123'
  c.core.upsertFlagV2(JSON.stringify({ key: 'f', is_enabled: true }))
  c.isEnabled('f', 'u1')
  assert.equal(c._impressions.length, 0)
  c.disconnect()
})

// --- _formatValue ----------------------------------------------------------

test('_formatValue serializes each value type', () => {
  const c = newClient()
  assert.equal(c._formatValue(null), 'null')
  assert.equal(c._formatValue(undefined), 'null')
  assert.equal(c._formatValue(true), 'true')
  assert.equal(c._formatValue(42), '42')
  assert.equal(c._formatValue('blue'), 'blue')
  assert.equal(c._formatValue({ a: 1 }), '{"a":1}')
  c.disconnect()
})

// --- Shutdown --------------------------------------------------------------

test('disconnect() flushes buffered impressions AND events', () => {
  const c = newClient()
  c._envId = 'env-123'
  c._impressions.push({ flag_key: 'f', user_id: 'u', value: 'true' })
  c._events.push({ event_key: 'g', user_id: 'u' })
  c.disconnect()

  const urls = fetchCalls.map((x) => x.url)
  assert.ok(urls.some((u) => /\/impressions$/.test(u)), 'impressions flushed on disconnect')
  assert.ok(urls.some((u) => /\/events$/.test(u)), 'events flushed on disconnect')
})

// --- Regression coverage for offline namespaces and polling lifecycle -------

test('cache namespaces match SHA-256 without exposing the credential', async () => {
    const { createHash } = await import('node:crypto');
    const { cacheNamespace } = require('../cache-key.js');
    for (const value of ['', 'abc', 'credential'.repeat(100), 'unicode-\u00e9-\ud83d\ude00']) {
        assert.equal(cacheNamespace(value), createHash('sha256').update(value).digest('hex'));
    }
});

test('persisted flags are isolated between credentials on the same server', async () => {
    const data = new Map();
    const storage = { getItem: k => data.get(k), setItem: (k, v) => data.set(k, v) };
    const prod = newClient({ sdkKey: 'production-secret', storage, reportImpressions: false });

    prod._applySnapshot([{key:'checkout',is_enabled:true}]);
    const stage = newClient({ sdkKey: 'staging-secret', storage, reportImpressions: false });

    await stage._hydrateFromCache();
    assert.equal(stage._flagCache.size, 0);
    assert.equal(prod._cacheKey.includes('production-secret'), false);
    prod.disconnect(); stage.disconnect();
});

test('polling bootstrap enables impressions and conversion tracking', async () => {
    const client = newClient();

    global.fetch = (async () => ({ok:true,headers:{get: () => 'poll-env'},json:async () => [{key:'checkout',is_enabled:true}]}));
    await client._pollSnapshot();
    assert.equal(client.isReady(), true);
    assert.equal(client._envId, 'poll-env');
    client.isEnabled('checkout','user');
    client.track('purchase','user');
    assert.equal(client._impressions.length, 1);
    assert.equal(client._events.length, 1);
    client.disconnect();
});

test('an outstanding poll cannot overwrite state after SSE takes over', async () => {
    const client = newClient({reportImpressions:false});

    let deliver;
    global.fetch = (() => new Promise(resolve => {deliver=resolve}));
    const pending = client._pollSnapshot();
    client._connectDeltas();
    client.sse.emit('connected',{environment_id:'sse-env'});
    client._applySnapshot([{key:'checkout',is_enabled:false}]);
    deliver({ok:true,json:async () => [{key:'checkout',is_enabled:true}]});
    await pending;
    assert.equal(client._flagCache.get('checkout').is_enabled, false);
    client.disconnect();
});

test('overlapping polls share a request and disconnect invalidates its response', async () => {
    const client = newClient({reportImpressions:false});

    let requests = 0, deliver;
    global.fetch = (() => {requests++; return new Promise(resolve => {deliver=resolve});});
    const first = client._pollSnapshot(), second = client._pollSnapshot();
    assert.equal(requests, 1);
    client.disconnect();
    deliver({ok:true,json:async () => [{key:'stale',is_enabled:true}]});
    await Promise.all([first,second]);
    assert.equal(client._flagCache.size, 0);
    assert.equal(client.isReady(), false);
});
