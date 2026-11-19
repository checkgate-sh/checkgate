---
title: "Web SDK (WebAssembly) — In-Browser Feature Flags with WASM"
description: "Use the Checkgate Web SDK to evaluate feature flags inside the browser via WebAssembly — no server round-trip on every call. Includes React and Vue examples."
---

# Web SDK (WebAssembly)

The web SDK compiles the Rust evaluation core to **WebAssembly** using [wasm-bindgen](https://github.com/rustwasm/wasm-bindgen). Flag evaluation runs at near-native speed entirely in the browser — no evaluation round-trips to your server.

## Installation

```bash
npm install @checkgate/web
# or
yarn add @checkgate/web
```

TypeScript declarations are included (since 0.1.31).

## Quick Start

```typescript
import { CheckgateWeb } from '@checkgate/web'

const client = new CheckgateWeb({
  serverUrl: 'https://flags.yourcompany.com',
  sdkKey: 'your-sdk-key',
})

// Connect once — downloads all flags via SSE
await client.connect()

// Evaluated locally in the browser: no request per decision
const enabled = client.isEnabled('dark-mode', currentUserId, {
  plan: userPlan,
  beta: 'true',
})
```

## API Reference

### `new CheckgateWeb(options)`

| Option | Type | Required | Description |
|--------|------|----------|-------------|
| `serverUrl` | `string` | Yes | Base URL of your Checkgate server |
| `sdkKey` | `string` | No | SDK key. Browsers cannot set headers on `EventSource`, so the stream sends it as a `?sdk_key=` query parameter (expect it in proxy access logs); polls send it as `Authorization: Bearer`. SDK keys are public by design |
| `reconnectDelayMs` | `number` | No | Initial reconnect delay, exponential backoff with jitter (default: 3000) |
| `maxReconnectDelayMs` | `number` | No | Maximum backoff delay (default: 30000) |
| `pollFallbackThreshold` | `number` | No | Failed reconnects before polling `/flags/snapshot` (default: 3) |
| `pollIntervalMs` | `number` | No | Poll interval while in fallback (default: 30000) |
| `connectTimeoutMs` | `number` | No | Retry a stream attempt without `connected` after this long (default: 30000) |
| `streamIdleTimeoutMs` | `number` | No | Reconnect a stream silent this long (default: 3 × the announced heartbeat; `0` disables; no idle timeout against servers before 0.1.31) |
| `reportImpressions` | `boolean` | No | Report each evaluation (flag key, user key, result) for analytics (default: `true`) |
| `impressionBatchSize` | `number` | No | Flush after this many buffered impressions (default: 50) |
| `impressionFlushIntervalMs` | `number` | No | Periodic impression flush (default: 10000) |
| `sendEvaluationContext` | `boolean` | No | Include user attributes in impressions (default: `false`) |
| `storage` | `Storage`-like | No | Persists the flag set for start-up before connecting. **Defaults to `localStorage`** when available; pass `null` to keep flag rules out of browser storage |
| `bootstrap` | `object` | No | A payload (or its `flags`) from [`@checkgate/ssr`](/ecosystem/ssr), loaded when `connect()` is called, so flags evaluate before the stream is established |

### `client.connect(): Promise<void>`

Opens an SSE connection, receives all flags, and populates the local WASM store. Resolves when bootstrap is complete. The connection is maintained for real-time updates.

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

A browser `EventSource` does not say why a connection failed, so after a failed attempt the SDK
asks `GET /flags/snapshot` (same key, at most once per attempt) whether the key is still accepted.

### `client.isEnabled(flagKey, userKey, attributes): boolean`

Synchronous, in-process evaluation using the WASM engine.

| Parameter | Type | Description |
|-----------|------|-------------|
| `flagKey` | `string` | Flag key to evaluate |
| `userKey` | `string` | Stable user identifier |
| `attributes` | `Record<string, string>` | User attributes for targeting |

Returns `false` for unknown flags.

### `client.disconnect(): void`

Closes the SSE connection.

## Usage with React

```tsx
// hooks/useFlag.ts
import { useEffect, useState } from 'react'
import { flags } from '../lib/flags' // shared client instance

export function useFlag(
  key: string,
  userId: string,
  attributes: Record<string, string> = {}
): boolean {
  const [enabled, setEnabled] = useState(() =>
    flags.isEnabled(key, userId, attributes)
  )

  useEffect(() => {
    // Re-evaluate when flags update
    const unsubscribe = flags.onChange(() => {
      setEnabled(flags.isEnabled(key, userId, attributes))
    })
    return unsubscribe
  }, [key, userId])

  return enabled
}

// Usage in a component
function CheckoutButton({ user }) {
  const newCheckout = useFlag('new-checkout-flow', user.id, {
    plan: user.plan,
  })

  return newCheckout ? <NewCheckout /> : <LegacyCheckout />
}
```

## Usage with Vue

```typescript
// composables/useFlag.ts
import { ref, onMounted, onUnmounted } from 'vue'
import { flags } from '../lib/flags'

export function useFlag(key: string, userId: string, attributes = {}) {
  const enabled = ref(flags.isEnabled(key, userId, attributes))

  let unsubscribe: (() => void) | null = null

  onMounted(() => {
    unsubscribe = flags.onChange(() => {
      enabled.value = flags.isEnabled(key, userId, attributes)
    })
  })

  onUnmounted(() => unsubscribe?.())

  return enabled
}
```

## WASM Loading

`connect()` initialises the WebAssembly module: in the browser, wasm-bindgen fetches
`checkgate_bg.wasm` relative to the SDK's own module URL, so your bundler must emit that file as an
asset next to the JavaScript (Vite does this without configuration). On the Node.js server runtime
the SDK reads the file from the package instead.

For webpack 5, add to your config:

```javascript
// webpack.config.js
module.exports = {
  experiments: {
    asyncWebAssembly: true,
  },
}
```

## Attributes Are Local

User attributes passed to `isEnabled()` stay in the browser: evaluation happens inside the WASM
module, and impressions omit attributes unless `sendEvaluationContext` is `true`. Impression
reporting itself is on by default and sends the flag key, user key and result of evaluations in
batches; set `reportImpressions: false` to send nothing but the connection and polls.
