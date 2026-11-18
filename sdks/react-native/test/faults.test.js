// Fault-injection suite for the React Native SDK: the real client with the
// real `react-native-sse` polyfill (the one the README tells apps to install),
// running over a streaming XMLHttpRequest like React Native's (rn-xhr.mjs), against
// ../conformance/fault-server.mjs. The JSI engine is replaced by a stand-in;
// the transport and every client rule are real. See sdks/conformance/README.md.
//
//   npx vitest run test/faults.test.js                       # CI seeds
//   CONFORMANCE_SEEDS=1-500 npx vitest run test/faults.test.js  # extended

import { test } from 'vitest'
import { ReactNativeXMLHttpRequest } from './rn-xhr.mjs'
import RNEventSource from 'react-native-sse'
import { CI_SEEDS, NAMED, generateScenario, runScenario, seedsFromEnv } from '../../conformance/scenario.mjs'
import { makeFakeCore } from '../../conformance/fake-core.mjs'

function bridge() {
  const core = makeFakeCore()
  return {
    upsertFlagV2: (flag) => core.upsert(flag),
    deleteFlag: (key) => core.delete(key),
    isEnabled: (key, user, attrs) => core.isEnabled(key, user, attrs),
    getVariant: () => 'null',
  }
}

globalThis.XMLHttpRequest = ReactNativeXMLHttpRequest
globalThis.EventSource = RNEventSource
globalThis.__CheckgateInternal = bridge()
const { CheckgateNativeClient } = await import('../index.js')
for (const level of ['log', 'warn', 'error']) console[level] = () => {}

const adapter = {
  create: (url, sdkKey, t) => {
    globalThis.__CheckgateInternal = bridge()
    return new CheckgateNativeClient({
      serverUrl: url, sdkKey, reportImpressions: false,
      reconnectDelayMs: t.reconnectDelayMs, maxReconnectDelayMs: t.maxReconnectDelayMs,
      pollFallbackThreshold: t.pollFallbackThreshold, pollIntervalMs: t.pollIntervalMs,
      connectTimeoutMs: t.connectTimeoutMs,
    })
  },
  connect: (c) => c.connect(),
  flags: (c) => new Map(c._flagCache),
  evaluate: (c, key, user) => c.isEnabled(key, user, { plan: 'pro' }),
  status: (c) => c.getStatus(),
  close: (c) => c.disconnect(),
}

const TIMEOUT = 30_000
for (const [name, scenario] of Object.entries(NAMED)) {
  test(`[react-native] ${name}`, async () => {
    await runScenario(adapter, { seed: 0, ...scenario }, { label: `rn-${name}` })
  }, TIMEOUT)
}
for (const seed of seedsFromEnv(CI_SEEDS)) {
  test(`[react-native] random seed ${seed}`, async () => {
    await runScenario(adapter, generateScenario(seed), { label: `rn-seed-${seed}` })
  }, TIMEOUT)
}
