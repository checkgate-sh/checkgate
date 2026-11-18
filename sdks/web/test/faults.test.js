// Fault-injection suite for the web SDK: the real client over a real HTTP
// stream (the `eventsource` package, made to behave like a browser's
// EventSource: error events carry no HTTP status) against
// ../conformance/fault-server.mjs. See sdks/conformance/README.md.
//
//   npx vitest run test/faults.test.js                       # CI seeds
//   CONFORMANCE_SEEDS=1-500 npx vitest run test/faults.test.js  # extended

import { test } from 'vitest'
import { EventSource as NodeEventSource } from 'eventsource'
import { CI_SEEDS, NAMED, generateScenario, runScenario, seedsFromEnv } from '../../conformance/scenario.mjs'
import { CheckgateWeb } from '../index.js'

/** A browser EventSource never says why a connection failed. */
class BrowserLikeEventSource extends NodeEventSource {
  addEventListener(type, listener, options) {
    if (type !== 'error') return super.addEventListener(type, listener, options)
    return super.addEventListener('error', (e) => listener({ type: 'error' }), options)
  }
}
globalThis.EventSource = BrowserLikeEventSource
for (const level of ['log', 'warn', 'error']) console[level] = () => {}

const adapter = {
  create: (url, sdkKey, t) => new CheckgateWeb({
    serverUrl: url, sdkKey, reportImpressions: false, storage: null,
    reconnectDelayMs: t.reconnectDelayMs, maxReconnectDelayMs: t.maxReconnectDelayMs,
    pollFallbackThreshold: t.pollFallbackThreshold, pollIntervalMs: t.pollIntervalMs,
    connectTimeoutMs: t.connectTimeoutMs,
  }),
  connect: (c) => c.connect(),
  flags: (c) => new Map(c._flagCache),
  evaluate: (c, key, user) => (c.core ? c.isEnabled(key, user, { plan: 'pro' }) : undefined),
  status: (c) => c.getStatus(),
  close: (c) => c.disconnect(),
}

const TIMEOUT = 30_000
for (const [name, scenario] of Object.entries(NAMED)) {
  test(`[web] ${name}`, async () => {
    await runScenario(adapter, { seed: 0, ...scenario }, { label: `web-${name}` })
  }, TIMEOUT)
}
for (const seed of seedsFromEnv(CI_SEEDS)) {
  test(`[web] random seed ${seed}`, async () => {
    await runScenario(adapter, generateScenario(seed), { label: `web-seed-${seed}` })
  }, TIMEOUT)
}
