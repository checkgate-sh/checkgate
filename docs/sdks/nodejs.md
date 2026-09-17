---
title: "Node.js SDK (NAPI) — In-Process Feature Flags for Node.js"
description: "Install and use the Checkgate Node.js SDK powered by NAPI-RS. isEnabled() is a synchronous in-memory call — no network round-trip — in Express, Next.js, and any Node.js server."
---

# Node.js SDK (NAPI)

The Node.js SDK uses [NAPI-RS](https://napi.rs/) to compile the Rust evaluation core into a native Node.js addon (`.node` file). Flag evaluation runs at native speed without crossing the JS/Rust FFI boundary unnecessarily.

## Installation

```bash
npm install @checkgate/node
# or
yarn add @checkgate/node
# or
pnpm add @checkgate/node
```

Pre-built binaries are included for:
- Linux x64 (glibc)
- Linux x64 (musl / Alpine)
- Linux arm64 (glibc)
- Linux arm64 (musl)
- macOS x64
- macOS arm64 (Apple Silicon)
- Windows x64

## Quick Start

```typescript
import { CheckgateClient } from '@checkgate/node'

const client = new CheckgateClient({
  serverUrl: 'https://flags.yourcompany.com',
  sdkKey: process.env.CHECKGATE_SDK_KEY,
})

// Connect and download flags (call once at startup)
await client.connect()

// Evaluate flags — no await, no network, sub-microsecond
const enabled = client.isEnabled('new-checkout-flow', userId, {
  email: user.email,
  plan: user.plan,
})
```

## API Reference

### `new CheckgateClient(options)`

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `serverUrl` | `string` | — | Base URL of your Checkgate server (required) |
| `sdkKey` | `string` | — | SDK key for authentication (Bearer) |
| `reconnectDelayMs` | `number` | `3000` | Initial SSE reconnect delay (exponential backoff with jitter from here) |
| `maxReconnectDelayMs` | `number` | `30000` | Maximum backoff delay between reconnects |
| `pollFallbackThreshold` | `number` | `3` | Consecutive failed SSE reconnects before falling back to polling `/flags/snapshot`. `Infinity` disables |
| `pollIntervalMs` | `number` | `30000` | Poll interval while in fallback mode |
| `reportImpressions` | `boolean` | `true` | Report evaluation events for analytics |
| `impressionBatchSize` | `number` | `50` | Flush after this many buffered events |
| `impressionFlushIntervalMs` | `number` | `10000` | Periodic flush interval |
| `sendEvaluationContext` | `boolean` | `false` | Include user attributes in reported impressions. Off by default so attributes never leave the process |
| `storage` | `StorageAdapter` | — | Pluggable persistence for offline hydrate-on-start (see [Resilience](#resilience-offline)) |

### `client.connect(): Promise<void>`

Connects to the server's SSE stream and downloads the current flag set. Resolves when the initial bootstrap is complete. Call this once at application startup.

Sets up automatic reconnection with exponential backoff on connection loss.

### `client.isEnabled(flagKey, userKey, attributes): boolean`

Evaluates a flag for a user. This is a synchronous, in-process call.

| Parameter | Type | Description |
|-----------|------|-------------|
| `flagKey` | `string` | The flag key to evaluate |
| `userKey` | `string` | Stable user identifier (used for rollout hashing) |
| `attributes` | `Record<string, string>` | User attributes for targeting rules |

Returns `false` if the flag does not exist.

### `client.getVariant(flagKey, userKey, attributes): { enabled, value } | null`

Evaluates a multi-variant flag and returns the full result — the on/off decision plus the
resolved `value` (boolean, string, number, or a decoded JSON object). Returns `null` if the flag
does not exist. Use this for string/integer/JSON flags and A/B variants.

### `client.getValue(flagKey, userKey, attributes, defaultValue): FlagValue`

Convenience wrapper that returns just the resolved value, or `defaultValue` when the flag is
missing.

```typescript
const color = client.getValue('checkout-button-color', userId, {}, 'blue') // string
```

### `client.track(eventKey, userKey, opts?): void`

Records a goal/conversion event for [A/B testing](/guide/experimentation#a-b-testing). Buffered
and reported asynchronously, like impressions. Use the **same `userKey`** you pass to
`getVariant()`/`isEnabled()` so conversions attribute to the right variant.

```typescript
client.track('checkout_complete', userId, { value: 49.99 })
```

`opts` accepts an optional numeric `value` (e.g. revenue) and a `context` object.

### `client.disconnect(): void`

Closes the SSE connection and cleans up resources (flushing any buffered impressions and events
first). Call on graceful shutdown.

## Resilience & Offline {#resilience-offline}

The SDK is built to keep evaluating through network trouble — the same resilience layer ships in
every Checkgate SDK:

- **Reconnect with backoff** — SSE reconnects use exponential backoff with jitter
  (`reconnectDelayMs` → `maxReconnectDelayMs`).
- **Poll fallback** — if SSE can't be established at all (e.g. a proxy blocking long-lived
  connections), after `pollFallbackThreshold` failed reconnects the SDK polls
  `GET /flags/snapshot` every `pollIntervalMs`, then switches back to SSE once it recovers.
- **Offline persistence** — pass a `storage` adapter (anything with `getItem`/`setItem`, e.g.
  `localStorage` or an async KV) and the SDK persists the flag set and re-hydrates it on cold
  start, so flags evaluate before — or entirely without — a connection.
- **SSR bootstrap** — pass a `bootstrap` snapshot (from [`@checkgate/ssr`](/ecosystem/ssr)) to seed
  the store synchronously on connect, for zero-flicker server-rendered apps.

Persisted snapshots are namespaced by server URL and a SHA-256 digest of the SDK key, so clients for different environments can share a storage adapter. The previous cache format is ignored on upgrade.

## Change listeners

Subscribe to live flag changes to re-render or invalidate caches when a flag actually changes
(bootstrap and reconnect resyncs are suppressed, so only genuine deltas fire):

```typescript
const unsubscribe = client.onChange((change) => {
  // change describes the upserted/deleted flag
  console.log('flag changed:', change)
})
```

::: tip Every SDK shares this surface
`getVariant`, `getValue`, `track`, the resilience options, and `bootstrap` are available in the
[Web](/sdks/web), [React Native](/sdks/react-native), and [Flutter](/sdks/flutter) SDKs too —
because all four wrap the same Rust evaluation core, flag decisions are identical across them.
:::

## Usage with Express

```typescript
import express from 'express'
import { CheckgateClient } from '@checkgate/node'

const flags = new CheckgateClient({
  serverUrl: process.env.CHECKGATE_URL!,
  sdkKey: process.env.CHECKGATE_SDK_KEY,
})

await flags.connect()

const app = express()

app.get('/checkout', (req, res) => {
  const newCheckout = flags.isEnabled(
    'new-checkout-flow',
    req.user.id,
    { plan: req.user.plan, email: req.user.email }
  )

  res.json({ ui: newCheckout ? 'v2' : 'v1' })
})

process.on('SIGTERM', () => flags.disconnect())
```

## Usage with Next.js

```typescript
// lib/flags.ts
import { CheckgateClient } from '@checkgate/node'

declare global {
  var checkgate: CheckgateClient | undefined
}

export async function getFlags(): Promise<CheckgateClient> {
  if (!global.checkgate) {
    global.checkgate = new CheckgateClient({
      serverUrl: process.env.CHECKGATE_URL!,
      sdkKey: process.env.CHECKGATE_SDK_KEY,
    })
    await global.checkgate.connect()
  }
  return global.checkgate
}
```

```typescript
// app/page.tsx (Server Component)
import { getFlags } from '@/lib/flags'

export default async function Page() {
  const flags = await getFlags()
  const showBanner = flags.isEnabled('promo-banner', 'anonymous', {})

  return showBanner ? <PromoBanner /> : null
}
```

## Passing Rules as JSON

When creating flags via the REST API, rules are a JSON array. The SDK accepts the same structure:

```typescript
// Via REST API
await fetch('/api/flags', {
  method: 'POST',
  body: JSON.stringify({
    key: 'beta-feature',
    is_enabled: true,
    rollout_percentage: 10,
    rules: [
      { attribute: 'plan', operator: 'equals', values: ['enterprise'] }
    ]
  })
})
```
