<p align="center">
  <img src="https://raw.githubusercontent.com/checkgate-sh/checkgate/main/assets/logo.svg" width="64" alt="Checkgate">
</p>

# @checkgate/node

Checkgate feature-flag SDK for Node.js servers. Flags are evaluated locally by
the Checkgate Rust engine, loaded as a native addon.

**Current release: 0.2.0.** Changes are listed in the
[changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md).

## What it does

- Connects once to your Checkgate server over Server-Sent Events (`GET /stream`),
  receives the full flag set, then applies live updates as flags change.
- Evaluates flags in process, synchronously: `isEnabled()`, `getVariant()`,
  `getValue()` and `getDetails()` make no network request per evaluation.
- Reconnects with exponential backoff and jitter. If the stream cannot be
  established repeatedly (for example, a proxy that blocks long-lived
  connections), it falls back to polling `GET /flags/snapshot` and switches
  back to the stream when it recovers.
- Keeps serving the last known flags while disconnected, and can persist them
  through an optional `storage` adapter so a restarted process evaluates before
  it reconnects.
- Reports evaluation impressions and `track()` goal events in batches, in the
  background (for experiments). User attributes are not sent unless you opt in.

## Installation

```bash
npm install @checkgate/node
```

The package ships prebuilt native binaries (see [Compatibility](#compatibility));
nothing is compiled on install.

## Minimal example

```js
import { CheckgateClient } from '@checkgate/node'

// One client per process, created at startup and shared.
const client = new CheckgateClient({
  serverUrl: process.env.CHECKGATE_URL, // e.g. http://localhost:3000
  sdkKey: process.env.CHECKGATE_SDK_KEY,
})

await client.connect() // resolves once the full flag set has arrived

// Synchronous, in memory: no network request per evaluation.
const attributes = { plan: 'pro' }
console.log(client.isEnabled('new-checkout', 'user-123', attributes)) // true / false
console.log(client.getValue('checkout-theme', 'user-123', attributes, 'light')) // the variant, or 'light' if the flag is missing
console.log(client.getDetails('new-checkout', 'user-123', attributes)) // { enabled, value, reason, ... }

client.disconnect() // on shutdown
```

CommonJS works the same way: `const { CheckgateClient } = require('@checkgate/node')`.

Create **one client per process** and share it (a module-level singleton). Each
client holds one stream to the server and a copy of the flags; creating one per
request opens a connection per request.

## Configuration

`new CheckgateClient(options)`:

| Option | Type | Default | Description |
|---|---|---|---|
| `serverUrl` | `string` | (required) | Base URL of your Checkgate server. |
| `sdkKey` | `string` | none | SDK key of the environment, sent as `Authorization: Bearer`. Leave unset only for a server in open (development) mode. |
| `reconnectDelayMs` | `number` | `3000` | Base reconnect delay; grows exponentially, with up to 30% jitter. |
| `maxReconnectDelayMs` | `number` | `30000` | Upper bound of the reconnect delay. |
| `pollFallbackThreshold` | `number` | `3` | Consecutive failed stream reconnects before polling `/flags/snapshot`. `Infinity` disables polling. |
| `pollIntervalMs` | `number` | `30000` | Poll interval while in poll fallback. |
| `connectTimeoutMs` | `number` | `30000` | A stream attempt without the server's `connected` event within this time is abandoned and retried. |
| `streamIdleTimeoutMs` | `number` | 3 × the server's heartbeat interval (45 s by default) | A connected stream with no event (heartbeats included) for this long is reconnected. No idle timeout against servers older than 0.1.31, which announce no heartbeat. `0` disables it. |
| `reportImpressions` | `boolean` | `true` | Report evaluation events for analytics and experiments. |
| `impressionBatchSize` | `number` | `50` | Flush buffered impressions once this many have accumulated. |
| `impressionFlushIntervalMs` | `number` | `10000` | Periodic flush interval for impressions and `track()` events. |
| `sendEvaluationContext` | `boolean` | `false` | Include user attributes in reported impressions. Off by default, so attributes never leave the process. |
| `storage` | `{ getItem(key), setItem(key, value) }` | none | Optional persistence adapter (sync or async). The last flag set is saved and loaded on the next start, so flags evaluate before or without a connection. |

## API overview

| Method | Returns | Description |
|---|---|---|
| `connect()` | `Promise<void>` | Opens the stream and resolves once the first complete flag set has arrived (from the stream, or from a poll in poll fallback). Call once at startup; calling it again while connecting returns the same promise. |
| `isEnabled(flagKey, userKey, attributes?)` | `boolean` | `false` if the flag does not exist. |
| `getVariant(flagKey, userKey, attributes?)` | `{ enabled, value } \| null` | `value` is a boolean, string, number or JSON value; `null` if the flag does not exist. |
| `getValue(flagKey, userKey, attributes?, defaultValue = null)` | `FlagValue` | The resolved value, or `defaultValue` if the flag does not exist. |
| `getDetails(flagKey, userKey, attributes?)` | `EvaluationDetails \| null` | New in 0.2.0. The same evaluation plus `reason` (`DISABLED`, `PREREQUISITE_FAILED`, `TARGETING_MATCH`, `ROLLOUT_EXCLUDED`, `SPLIT`, `DEFAULT`, `STATIC`) and, when they apply, `ruleIndex`, `rolloutBucket`, `variantIndex` and `prerequisites`. |
| `track(eventKey, userKey, { value?, context? }?)` | `void` | Records a goal event for experiments; buffered and sent in the background. Use the same `userKey` as for evaluations. |
| `onChange(callback)` | unsubscribe function | Called with the key of each flag that changes after the initial bootstrap. |
| `getStatus()` | `{ state, lastSyncAt, error }` | Connection state: `idle`, `connecting`, `streaming`, `reconnecting`, `polling`, `auth_failed` or `closed`. |
| `onStatusChange(callback)` | unsubscribe function | New in 0.2.0. Called with `getStatus()` whenever the state changes, and after each poll while in poll fallback. |
| `getHealth()` | `CheckgateHealth` | New in 0.2.0. Diagnostics (schema 1): `ready`, `source`, `flagCount`, `configAgeMs`, revisions, reconnect and poll counters, `lastError`, `offlineCache`. Local state only. |
| `isReady()` / `isPolling()` | `boolean` | Whether the bootstrap has completed; whether poll fallback is active. |
| `disconnect()` | `void` | Closes the stream, stops polling, flushes buffered impressions and events. |

`userKey` is a stable user identifier; percentage rollouts and weighted
variants hash it. `attributes` is a `Record<string, string>` read by targeting
rules. TypeScript declarations are included (`index.d.ts`).

## Initialization, health and shutdown

```js
import { CheckgateClient, CheckgateAuthError } from '@checkgate/node'

const client = new CheckgateClient({
  serverUrl: process.env.CHECKGATE_URL,
  sdkKey: process.env.CHECKGATE_SDK_KEY,
})

client.onStatusChange(({ state, error }) => console.log('checkgate:', state, error?.message ?? ''))

// connect() keeps retrying while the server is unreachable, so bound the wait.
let timer
const timeout = new Promise((resolve) => { timer = setTimeout(resolve, 5000, false) })
try {
  const ready = await Promise.race([client.connect().then(() => true), timeout])
  if (!ready) console.warn('Flags not loaded yet: serving defaults, still connecting')
} catch (err) {
  if (!(err instanceof CheckgateAuthError)) throw err
  console.error(`SDK key rejected (HTTP ${err.status})`) // not retried
} finally {
  clearTimeout(timer)
}

// e.g. behind a readiness probe
const health = client.getHealth()
const fresh = health.ready && health.configAgeMs !== null && health.configAgeMs < 5 * 60_000
console.log({ fresh, state: health.state, source: health.source, flagCount: health.flagCount })

process.once('SIGTERM', () => client.disconnect())
```

## Error handling and lifecycle

- **`connect()` rejects** with `CheckgateAuthError` (`err.status` is 401 or
  403) when the server rejects the SDK key, and with an `Error` if
  `disconnect()` is called before it completes. While the server is unreachable
  it does not reject; it keeps retrying. A caller that does not await it gets
  no unhandled rejection.
- **A rejected key is not retried.** The state becomes `auth_failed`, the flags
  already loaded keep being served, and calling `connect()` again retries.
- **Before the first flag set** (and without a hydrated `storage` snapshot),
  evaluations return `false` / `null` / `defaultValue` and log a warning.
- **While disconnected**, evaluations answer from the last known flags.
  `Date.now() - getStatus().lastSyncAt` is how old they may be; `getHealth()`
  has `configAgeMs` and `lastError` (`{ message, kind, at }`, not cleared by a
  later success).
- **Attribute values must be strings.** A non-string value for an attribute
  that a rule reads makes `isEnabled()` throw and `getVariant()`,
  `getValue()` and `getDetails()` answer as if the flag were missing.
  Attributes no rule reads are not checked.
- **Listeners** passed to `onChange()` and `onStatusChange()` that throw are
  logged and do not affect the client.
- **Shutdown:** call `disconnect()` once on graceful shutdown. It flushes
  buffered impressions and events (best effort, not awaited) and sets the
  state to `closed`. A later `connect()` starts again.
- Impression and event reporting is best effort: a batch that fails to send
  is dropped, not retried, and the buffers keep at most the newest 10,000
  entries.

## Compatibility

- **Node.js:** the package declares no `engines` range. CI tests the SDK with
  its native addon on Node.js 22, 24 and 26. Node.js 20 reached end of life on
  2026-04-30 and is not tested.
- **Prebuilt binaries** (one package, no install script):

  | Platform | Binary |
  |---|---|
  | Linux x64, glibc | `checkgate.linux-x64-gnu.node` |
  | Linux arm64, glibc | `checkgate.linux-arm64-gnu.node` |
  | Linux x64, musl (Alpine) | `checkgate.linux-x64-musl.node` |
  | macOS x64 | `checkgate.darwin-x64.node` |
  | macOS arm64 (Apple Silicon) | `checkgate.darwin-arm64.node` |
  | Windows x64 | `checkgate.win32-x64-msvc.node` |

  Other platforms (for example Linux arm64 with musl, or Windows arm64) have
  no binary, and loading the package fails there.
- **Server-only.** It is a native addon for Node.js; it does not run in
  browsers or edge runtimes. In Next.js, keep it out of the server bundle with
  `serverExternalPackages: ['@checkgate/node']`.
- **Servers:** the SDK protocol is unchanged in 0.2.0, so this SDK works with
  0.1.31 and 0.2.0 servers. Against servers older than 0.1.31, polls replace the
  whole flag set (no per-flag revision ordering) and there is no stream idle
  timeout.

Related packages: `@checkgate/openfeature-provider` (OpenFeature server
provider) and `@checkgate/node-integrations` (Express, Fastify, NestJS), both
new in 0.2.0.

## Security considerations

- SDK keys are tied to one environment and can only reach the SDK endpoints
  (stream, snapshot, reading flags, reporting impressions and events). Still,
  keep the key in configuration or a secret store rather than in source.
- The key travels in the `Authorization` header: use an `https://` `serverUrl`
  outside local development.
- Targeting attributes stay in the process unless you set
  `sendEvaluationContext: true`. User keys are sent with impressions and
  `track()` events; set `reportImpressions: false` to stop impressions.
- `getHealth()` and `CheckgateAuthError` messages never contain the SDK key,
  user keys or attributes.
- With `storage`, the persisted flag set is stored under a key namespaced by
  the server URL and a SHA-256 digest of the SDK key (not the key itself). The
  flag rules themselves are stored as plain JSON; protect the storage
  accordingly.

## Documentation

- [Node.js SDK guide](https://checkgate-sh.github.io/checkgate/sdks/nodejs)
- [SDK diagnostics](https://checkgate-sh.github.io/checkgate/operations/sdk-diagnostics) (`getHealth()`, `onStatusChange()`)
- [Upgrading to 0.2.0](https://checkgate-sh.github.io/checkgate/operations/upgrading-to-0-2-0)
- [OpenFeature provider](https://checkgate-sh.github.io/checkgate/sdks/openfeature) and [Express, Fastify and NestJS](https://checkgate-sh.github.io/checkgate/sdks/node-frameworks)
- [Changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md)
- [Source and issues](https://github.com/checkgate-sh/checkgate)

## License

Apache-2.0
