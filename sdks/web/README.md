<p align="center">
  <img src="https://raw.githubusercontent.com/checkgate-sh/checkgate/main/assets/logo.svg" width="64" alt="Checkgate">
</p>

# @checkgate/web

Browser SDK for [Checkgate](https://github.com/checkgate-sh/checkgate), the
self-hosted feature-flag platform. Flags are evaluated locally in the browser by
the Checkgate Rust engine compiled to WebAssembly.

**Current release: 0.2.0.** No API changes since 0.1.31; the package is
versioned with the platform release. See the
[changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md).

## What it does

- Opens one Server-Sent Events (SSE) stream to your Checkgate server. On every
  connect the server sends the complete current flag set of the SDK key's
  environment, then each change as it happens.
- Evaluates flags synchronously in WebAssembly: no network request per
  evaluation. User attributes stay in the browser.
- Keeps the flag set current: live updates, reconnection with backoff, and a
  polling fallback when the stream cannot be established.
- Persists the last flag set (in `localStorage` by default) so the next page
  load can evaluate before the stream is up, and accepts a server-rendered
  `bootstrap` payload from [`@checkgate/ssr`](https://www.npmjs.com/package/@checkgate/ssr).
- Reports impressions (flag key, user key, result) and `track()` conversion
  events for experiments, batched and best-effort.
- Exports the same engine for server and edge runtimes as `@checkgate/web/core`
  (used by `@checkgate/ssr` and `@checkgate/edge`).

## Installation

```bash
npm install @checkgate/web
```

The package ships the compiled `.wasm` and TypeScript declarations; no Rust
toolchain is needed.

## Quick start

```js
import { CheckgateWeb } from '@checkgate/web'

const checkgate = new CheckgateWeb({
  serverUrl: 'https://flags.example.com', // no trailing slash
  sdkKey: 'sk_live_...',                  // the environment's SDK key (public, see Security)
})

// Loads the WebAssembly engine, opens the stream and resolves once the full
// flag set has arrived.
await checkgate.connect()

// Flag key, a stable user key, string attributes for targeting rules.
if (checkgate.isEnabled('new-checkout', 'user-123', { plan: 'pro' })) {
  // render the new checkout
}

const color = checkgate.getValue('checkout-color', 'user-123', {}, 'blue')

checkgate.onChange((flagKey) => {
  // a flag changed after the initial load: re-render
})
```

### A new user gets the current configuration

The server builds the initial flag set for each stream from its database at the
moment the client connects; it does not replay a history of change events. A
user who opens your page a minute or a month after a flag was changed receives
the flag as it is now, the same as users who were connected when it changed.
Nobody has to be online when you change a flag.

Flags restored from `localStorage` or passed as `bootstrap` are used only until
that initial set arrives (the `ready` event). The set then replaces them,
including removing flags that were deleted on the server.

## Configuration

`new CheckgateWeb(options)`:

| Option | Type | Default | Description |
|---|---|---|---|
| `serverUrl` | `string` | required | Base URL of the Checkgate server. |
| `sdkKey` | `string` | none | SDK key of the environment. The stream sends it as `?sdk_key=` (`EventSource` cannot set headers); polls and reports send `Authorization: Bearer`. The server rejects requests without a valid key. |
| `reconnectDelayMs` | `number` | `3000` | Base reconnect delay; doubles per failed attempt. |
| `maxReconnectDelayMs` | `number` | `30000` | Upper bound of the reconnect delay (before jitter). |
| `pollFallbackThreshold` | `number` | `3` | Consecutive failed stream attempts before polling `GET /flags/snapshot`. `Infinity` disables polling. |
| `pollIntervalMs` | `number` | `30000` | Poll interval while polling. |
| `connectTimeoutMs` | `number` | `30000` | A stream attempt without the server's `connected` event within this time is abandoned and retried. |
| `streamIdleTimeoutMs` | `number` | 3 x server heartbeat | A connected stream with no event (heartbeats included) for this long is treated as dead and reconnected. The server announces its heartbeat interval (15 s by default, so 45 s). `0` disables. Servers older than 0.1.31 announce none, so no idle timeout applies to them. |
| `reportImpressions` | `boolean` | `true` | Report each evaluation (flag key, user key, result) for analytics. |
| `impressionBatchSize` | `number` | `50` | Flush impressions (and events) once this many are buffered. |
| `impressionFlushIntervalMs` | `number` | `10000` | Periodic flush interval for impressions and events. |
| `sendEvaluationContext` | `boolean` | `false` | Include user attributes as `context` in impressions. Off, attributes never leave the browser. |
| `storage` | `{ getItem, setItem }` or `null` | `localStorage` when available | Where the last flag set is persisted. Sync or async adapters both work. `null` persists nothing. |
| `bootstrap` | flag array, `{ flags }` or `null` | `null` | A flag set loaded by `connect()` before the stream is established, typically the payload from `@checkgate/ssr`. When given, `storage` is not read. |

## API

| Member | Description |
|---|---|
| `connect(): Promise<void>` | Loads the WASM engine, restores `bootstrap` or stored flags, opens the stream. Resolves when the full flag set has been received (stream) or a poll succeeded. Calling it again while pending returns the same promise. |
| `isEnabled(flagKey, userKey, attributes?): boolean` | Synchronous evaluation. `false` for unknown flags and before any flags are loaded. Attributes that are not strings are ignored. |
| `getVariant(flagKey, userKey, attributes?)` | `{ enabled, value }` for multi-variant flags, or `null` if the flag does not exist or nothing is loaded. |
| `getValue(flagKey, userKey, attributes?, defaultValue = null)` | The resolved value, or `defaultValue` when `getVariant()` would return `null`. |
| `onChange(cb): () => void` | `cb(flagKey)` for each flag created, updated or deleted after the initial load. Not called for the initial load; after a reconnect, only for flags that changed meanwhile. Returns an unsubscribe function. |
| `track(eventKey, userKey, { value?, context? }?)` | Records a conversion event for experiments. Use the same `userKey` as for evaluation. |
| `getStatus()` | `{ state, lastSyncAt, error }`, see below. |
| `isReady(): boolean` | Whether the full flag set has been received from the server. |
| `isPolling(): boolean` | Whether the client is currently polling instead of streaming. |
| `disconnect(): void` | Closes the stream, stops polling and timers, flushes pending impressions and events. |
| `CheckgateAuthError` | Error class (`name`, `status` 401 or 403) for a rejected SDK key. |

`getStatus().state` is one of `idle` (before `connect()`), `connecting`,
`streaming` (flags current), `reconnecting`, `polling`, `auth_failed` or
`closed`. `lastSyncAt` is when the client last confirmed its flags were current
(initial set, successful poll, or any stream event including heartbeats), in ms
since the epoch, or `null`. Flags are served from the last known set in every
state; `Date.now() - lastSyncAt` is how old they may be.

### Server-side core (`@checkgate/web/core`)

```js
import { createWasmCore } from '@checkgate/web/core'

const core = await createWasmCore() // Node and browser: no argument
core.upsertFlag(JSON.stringify(flag)) // flags as served by GET /flags/snapshot
core.isEnabled('new-checkout', 'user-123', { plan: 'pro' })
core.getValue(key, userKey, attrs, defaultValue)
core.getVariant(key, userKey, attrs)
core.clear()
```

`createWasmCore(wasmInput?)` accepts a `WebAssembly.Module`, bytes, `Response`
or `URL` for runtimes that load `.wasm` themselves (on Cloudflare Workers, import
`@checkgate/web/dist/checkgate_bg.wasm` and pass it). Each call returns an
independent flag store. This is the engine `@checkgate/ssr` and
`@checkgate/edge` use.

## Using it with React

There is no separate React package and this package has no React bindings.
Create one client for the app and re-render from `onChange` and from
`connect()` settling (`onChange` does not fire for the initial load):

```js
// flags.js
import { CheckgateWeb } from '@checkgate/web'

export const flags = new CheckgateWeb({ serverUrl: 'https://flags.example.com', sdkKey: 'sk_live_...' })
export const flagsReady = flags.connect()
```

```js
// use-flag.js
import { useEffect, useReducer } from 'react'
import { flags, flagsReady } from './flags'

export function useFlag(flagKey, userKey, attributes) {
  const [, rerender] = useReducer((n) => n + 1, 0)
  useEffect(() => {
    let active = true
    const update = () => { if (active) rerender() }
    flagsReady.then(update, update)
    const off = flags.onChange(update)
    return () => { active = false; off() }
  }, [])
  return flags.isEnabled(flagKey, userKey, attributes)
}
```

For server-rendered React (Next.js and others) use `@checkgate/ssr`: it
evaluates on the server, embeds the result in the HTML, and seeds this client
through the `bootstrap` option so the first paint matches the server render.

## Local storage

- **What is stored:** the flag definitions the client holds, as one JSON object
  keyed by flag key, exactly as the server sent them (state, rollout, targeting
  rules with their values, segments expanded into rules, variants). No user
  keys, attributes, evaluation results or the SDK key.
- **Key:** `checkgate:flags:v2:<serverUrl>:<hash>`, where `<hash>` is the
  SHA-256 hex of the SDK key (of `session` when there is none). Different
  servers and keys do not share an entry, and the raw key is not in the storage
  key.
- **Written** after the initial flag set arrives, after each live change, and
  after each poll that changed something.
- **Read** once per `connect()`, after the WASM engine loads, and only when no
  `bootstrap` flags were passed. Stored flags are evaluated at once (before
  `connect()` resolves; `isReady()` is still `false`) until the server's flag set
  replaces them.
- There is no expiry. Storage errors are logged with `console.warn` and
  otherwise ignored. Pass `storage: null` to keep flag definitions out of
  browser storage, or any `{ getItem, setItem }` adapter (for example
  `sessionStorage`).

## Reconnection

- A stream error, a connect timeout (`connectTimeoutMs`) or an idle stream
  (`streamIdleTimeoutMs`) closes the stream; the SDK reconnects itself (not with
  the browser's built-in `EventSource` retry) after
  `reconnectDelayMs x 2^attempt`, capped at `maxReconnectDelayMs`, plus up to 30%
  random jitter.
- After `pollFallbackThreshold` consecutive failures it also polls
  `GET /flags/snapshot` every `pollIntervalMs` while still retrying the stream.
  A successful stream connection stops polling and resets the backoff.
- Every reconnect receives the full current flag set again. It is staged and
  swapped in whole when complete, so evaluations keep using the previous set
  meanwhile and a replay cut short never goes live. Change events carry
  revisions; late or duplicate ones are ignored.
- A browser does not expose the HTTP status of a failed `EventSource`. After an
  attempt that never connected, the SDK asks `GET /flags/snapshot` with the same
  key (at most once per attempt); a 401 or 403 stops all retries.

## Errors and lifecycle

- `connect()` rejects with `CheckgateAuthError` when the server rejects the key
  (401/403). The client stops retrying, keeps serving the flags it has, and
  `getStatus()` reports `auth_failed`. Call `connect()` again to retry.
- `connect()` rejects with an `Error` if `disconnect()` is called first, or if
  the WASM engine fails to load. It does **not** reject while the server is
  unreachable; it keeps retrying. Race it with a timeout if your startup cannot
  wait, and evaluate meanwhile (stored or bootstrap flags, else defaults).
- Calling `connect()` without awaiting it never produces an unhandled
  rejection.
- Before any flags are loaded, `isEnabled()` returns `false` and `getVariant()`
  returns `null` (with a `console.warn`).
- An exception thrown by an `onChange` listener is caught and logged.
- Impressions and events are buffered (at most 10,000 each, oldest dropped),
  sent in batches of up to 500 with `fetch(..., { keepalive: true })`, and not
  retried on failure. `track()` before the stream has connected drops the event
  with a warning, because the environment is not known yet.
- `disconnect()` flushes pending reports and closes everything. Flags already
  loaded stay available for evaluation; a later `connect()` opens a new stream
  and receives the current set.

## Security considerations

- **The SDK key is visible to anyone who loads your page.** Checkgate has one
  kind of SDK key, bound to one environment, and treats it as public. It can only:
  open the stream and read the flags snapshot of its environment, read that
  environment's flag definitions, and submit impressions and events for it. It
  cannot create or change flags, read other environments, or call any other
  API; those requests get 403. Use a separate key per app so you can revoke one
  without affecting others; a revoked key gets 401 and its open streams close.
- **Flag definitions reach the browser.** Every flag of the environment,
  including targeting rule values and segment contents, is sent to the client
  and (by default) stored in `localStorage`. Do not put secrets or personal data
  you would not publish into flags of an environment whose key ships to
  browsers.
- **The key appears in stream URLs** (`?sdk_key=`), so it can show up in proxy
  and server access logs. Other requests use the `Authorization` header.
- **Client-side flags are not access control.** A user can change what the
  browser evaluates. Enforce authorization on your server.
- Your Content Security Policy must allow the Checkgate server origin in
  `connect-src`.

## Compatibility

- ES module package with TypeScript declarations; no CommonJS build.
- Needs `WebAssembly`, `EventSource` and `fetch`. In the browser, the engine
  fetches `checkgate_bg.wasm` relative to the module
  (`new URL('checkgate_bg.wasm', import.meta.url)`), so your bundler or server
  must serve that file next to the SDK's JavaScript.
- `@checkgate/web/core` also runs on Node.js (it reads the `.wasm` from the
  package) and on runtimes that accept a `WebAssembly.Module`.
- Tested in CI: unit and fault-injection suites on Node.js 22 with a stubbed
  engine, and a live end-to-end run of the real client and WASM engine against a
  real server, in Node.js with an `EventSource` polyfill. It is not tested in a
  real browser in CI.
- Works with servers older than 0.1.31 (no heartbeat idle timeout, polls without
  revisions).

## Documentation

- [Web SDK guide](https://checkgate-sh.github.io/checkgate/sdks/web)
- [SSR and bootstrap](https://checkgate-sh.github.io/checkgate/ecosystem/ssr)
- [SDK keys and authentication](https://checkgate-sh.github.io/checkgate/api-reference#authentication)
- [SSE stream protocol](https://checkgate-sh.github.io/checkgate/api-reference#sse-stream)
- [Reliability and sync](https://checkgate-sh.github.io/checkgate/operations/reliability)
- [SDK diagnostics](https://checkgate-sh.github.io/checkgate/operations/sdk-diagnostics)
- [Changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md)

## License

Apache-2.0
