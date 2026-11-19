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

Requires Node.js 20 or later.

Pre-built binaries are included for:
- Linux x64 (glibc)
- Linux x64 (musl / Alpine)
- Linux arm64 (glibc)
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

// Evaluate flags — no await, no network call
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
| `connectTimeoutMs` | `number` | `30000` | A stream attempt without the server's `connected` event by then is retried |
| `streamIdleTimeoutMs` | `number` | 3 × announced heartbeat | A connected stream silent this long is reconnected. No idle timeout against servers before 0.1.31; `0` disables |
| `reportImpressions` | `boolean` | `true` | Report evaluation events for analytics |
| `impressionBatchSize` | `number` | `50` | Flush after this many buffered events |
| `impressionFlushIntervalMs` | `number` | `10000` | Periodic flush interval |
| `sendEvaluationContext` | `boolean` | `false` | Include user attributes in reported impressions. Off by default so attributes never leave the process |
| `storage` | `CheckgateStorage` | — | Pluggable persistence for offline hydrate-on-start (see [Resilience](#resilience-offline)) |

### `client.connect(): Promise<void>`

Connects to the server's SSE stream and downloads the current flag set. Resolves when the initial bootstrap is complete. Call this once at application startup.

Sets up automatic reconnection with exponential backoff on connection loss.

### Connection health

`client.getStatus()` returns `{ state, lastSyncAt, error }`:

| `state` | Meaning |
|---|---|
| `'idle'` | Created; `connect()` not called yet |
| `'connecting'` | First connection attempt in progress |
| `'streaming'` | Live stream; flags are current |
| `'reconnecting'` | Stream lost; retrying with backoff (flags: last known) |
| `'polling'` | Stream keeps failing; polling `/flags/snapshot` |
| `'auth_failed'` | The server rejected the SDK key (401/403); not retrying |
| `'closed'` | After `disconnect()` |

`lastSyncAt` is when the client last confirmed its flags are current (a bootstrap, a poll, or any
event on a live stream, the server's heartbeats included), in ms since the epoch. Flags are always
served from the last known state; `Date.now() - lastSyncAt` is how old they may be. A stream that
receives nothing for 3 × the server's heartbeat interval (45 s by default) is dropped and
reconnected, so a silently dead connection (lost NAT mapping, half-open TCP) no longer looks healthy.

A rejected key is not retried: `connect()` rejects with a `CheckgateAuthError` (`err.status` is 401 or 403; the
key is never in the message), the last known flags keep being served, and calling `connect()` again
retries. `connect()` also rejects if `disconnect()` is called before it completes. While the server is
unreachable it keeps retrying; race it with a timeout if startup cannot wait.

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

Persisted snapshots are namespaced by server URL and a SHA-256 digest of the SDK key, so clients for different environments can share a storage adapter. The previous cache format is ignored on upgrade.

## Change listeners

Subscribe to flag changes to re-render or invalidate caches. The callback receives the key of
each flag that actually changed: the initial bootstrap does not fire it, a duplicate event does
not fire it, and after a reconnect it fires only for flags that changed while disconnected.

```typescript
const unsubscribe = client.onChange((flagKey) => {
  console.log('flag changed:', flagKey)
})
```

::: tip Every SDK shares this surface
`getVariant`, `getValue`, `track`, `getStatus()` and the reconnect / poll-fallback behaviour are
available in the [Web](/sdks/web), [React Native](/sdks/react-native), and [Flutter](/sdks/flutter)
SDKs too — because all four wrap the same Rust evaluation core, flag decisions are identical across
them. Option names differ slightly per SDK (Flutter uses `Duration`s), and the `bootstrap` option
for server-rendered apps is Web-only; see each SDK's page.
:::

::: info Server compatibility
With a 0.1.31 server, the SDK polls `/flags/snapshot?v=2` and applies polls and stream replays in
revision order per flag, so a late replay cannot roll a flag back. This is per flag, not atomic
across flags. Against an older server polls replace the whole flag set as before, and there is no
stream idle timeout (older servers announce no heartbeat). See
[Configuration consistency](/operations/configuration-consistency).
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
  var checkgate: Promise<CheckgateClient> | undefined
}

// One client per server process. The connect promise is shared, so concurrent
// requests during a cold start wait for the same bootstrap. connect() keeps
// retrying while the server is unreachable, so cap how long a request waits;
// until the first bootstrap the client serves `false` (or stored flags).
export function getFlags(timeoutMs = 3000): Promise<CheckgateClient> {
  if (!global.checkgate) {
    const client = new CheckgateClient({
      serverUrl: process.env.CHECKGATE_URL!,
      sdkKey: process.env.CHECKGATE_SDK_KEY,
    })
    const ready = client.connect().then(() => client)
    const timeout = new Promise<CheckgateClient>((resolve) =>
      setTimeout(() => resolve(client), timeoutMs).unref())
    global.checkgate = Promise.race([ready, timeout])
  }
  return global.checkgate
}
```

`@checkgate/node` is a native addon, so keep it out of Next.js's server bundle:

```typescript
// next.config.ts
export default { serverExternalPackages: ['@checkgate/node'] }
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
