<p align="center">
  <img src="https://raw.githubusercontent.com/checkgate-sh/checkgate/main/assets/logo.svg" width="64" alt="Checkgate">
</p>

# @checkgate/edge

Edge and server evaluator for [Checkgate](https://github.com/checkgate-sh/checkgate),
the self-hosted feature-flag platform: fetches a flag snapshot, caches it with a
TTL and optional stale-while-revalidate, and evaluates locally with the
Checkgate engine.

**Current release: 0.2.0.** No API changes since 0.1.31; the package is
versioned with the platform release. See the
[changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md).

## What it does

- Fetches `GET /flags/snapshot` from your Checkgate server with the SDK key as
  a Bearer token and loads every flag into an evaluation engine you inject.
- Serves evaluations from memory: no network request per evaluation. Within the
  TTL nothing is fetched; within the stale-while-revalidate window the old
  snapshot is served while a refresh runs in the background.
- Fails open: once a snapshot has loaded, a failed refresh keeps serving it.
- Does not evaluate flags itself. In production the engine is
  `@checkgate/web/core`, the same WebAssembly build of the Rust engine the
  browser SDK uses, so results match the other SDKs.

It polls; it does not hold a stream. A flag change becomes visible at the next
refresh, so at most `ttlSeconds` (plus the stale window) after the change. It
does not report impressions or events.

## Installation

```bash
npm install @checkgate/edge @checkgate/web
```

`@checkgate/edge` has no dependencies. `@checkgate/web` provides the engine
(`@checkgate/web/core`); any object with the same methods works (see `core`
below).

## Quick start

Node.js (or any runtime where `createWasmCore()` can load the `.wasm` itself):

```js
import { CheckgateEdge } from '@checkgate/edge'
import { createWasmCore } from '@checkgate/web/core'

const edge = new CheckgateEdge({
  serverUrl: 'https://flags.example.com',
  sdkKey: process.env.CHECKGATE_SDK_KEY,
  core: await createWasmCore(),
  ttlSeconds: 30,
  staleWhileRevalidateSeconds: 60,
})

// Per request:
await edge.ensureFresh()
const showIt = edge.isEnabled('new-homepage', userKey, { plan: 'pro' })
const color = edge.getValue('checkout-color', userKey, {}, 'blue')
```

Cloudflare Workers: Wrangler compiles a `.wasm` import into a
`WebAssembly.Module`, which `createWasmCore()` accepts. Keep the instance in a
module global so a warm isolate reuses it, and hand background refreshes to
`ctx.waitUntil`:

```js
import { CheckgateEdge } from '@checkgate/edge'
import { createWasmCore } from '@checkgate/web/core'
import wasmModule from '@checkgate/web/dist/checkgate_bg.wasm'

let edge = null

export default {
  async fetch(request, env, ctx) {
    edge ??= new CheckgateEdge({
      serverUrl: env.CHECKGATE_URL,
      sdkKey: env.CHECKGATE_SDK_KEY,
      core: await createWasmCore(wasmModule),
      staleWhileRevalidateSeconds: 60,
    })
    await edge.ensureFresh({ waitUntil: (p) => ctx.waitUntil(p) })
    const userKey = request.headers.get('x-user-id') ?? 'anonymous'
    return Response.json({ newHomepage: edge.isEnabled('new-homepage', userKey) })
  },
}
```

A complete Worker with a Cron Trigger that keeps the snapshot warm is in
[`examples/cloudflare-worker`](https://github.com/checkgate-sh/checkgate/tree/main/edge/examples/cloudflare-worker).

## Configuration

`new CheckgateEdge(options)`:

| Option | Type | Default | Description |
|---|---|---|---|
| `serverUrl` | `string` | required | Checkgate server base URL. Trailing slashes are removed. |
| `sdkKey` | `string` | required | SDK key of the environment, sent as `Authorization: Bearer`. |
| `core` | object | required | Evaluation engine with `upsertFlag(json)`, `clear()`, `isEnabled()`, `getValue()`, `getVariant()`; normally `await createWasmCore()` from `@checkgate/web/core`. |
| `ttlSeconds` | `number` | `30` | How long a loaded snapshot is fresh. |
| `staleWhileRevalidateSeconds` | `number` | `0` | Extra window after the TTL in which the stale snapshot is served and refreshed in the background. |
| `fetchImpl` | `fetch`-compatible function | `globalThis.fetch` | HTTP client. Use it to add a timeout (see below). |
| `now` | `() => number` | `Date.now` | Clock in ms (for tests). |
| `snapshotPath` | `string` | `/flags/snapshot` | Path appended to `serverUrl`. |

## API

| Member | Description |
|---|---|
| `ensureFresh({ waitUntil }?)` | Never loaded: wait for a refresh. Within the TTL: nothing. Within TTL + stale window: start a background refresh (passed to `waitUntil` if given) and return. Beyond: wait for a refresh, keeping the old snapshot if it fails. |
| `refresh()` | Fetch and load the snapshot now. Concurrent calls share one request. Resolves to `{ count, refreshed, status?, error? }`. |
| `isEnabled(key, userKey, attrs?)` | Delegates to the core. |
| `getValue(key, userKey, attrs?, defaultValue = null)` | Delegates to the core. |
| `getVariant(key, userKey, attrs?)` | Delegates to the core: `{ enabled, value }` or `null`. |
| `snapshotFlags()` | Copy of the raw flag array last loaded (empty before the first load); pass it to `@checkgate/ssr`'s `buildBootstrap()`. |
| `isLoaded`, `flagCount`, `lastLoadedAt` | Whether a snapshot has loaded, its flag count, and when (ms, or `null`). |
| `ageMs()` | Age of the loaded snapshot in ms (`Infinity` before the first load). |

## Errors and lifecycle

- The constructor throws if `serverUrl`, `sdkKey` or a usable `core` is missing,
  or no `fetch` is available.
- The **first** load has nothing to fall back to: `refresh()` and
  `ensureFresh()` reject on a network error, a non-2xx status or a body that is
  not a JSON array of flags.
- After the first load, `ensureFresh()` never throws and the previous snapshot
  keeps serving. `refresh()` resolves with `refreshed: false` for a network
  error (`error`), a non-2xx status (`status`) or a body that is not an array; it
  rejects only if the body is not valid JSON. This includes 401 and 403: a
  revoked key keeps serving the last snapshot. Watch `ageMs()` if you need to
  detect that.
- Requests beyond TTL + stale window wait for the refresh, and the package sets
  no timeout of its own. Pass a `fetchImpl` that does:

  ```js
  fetchImpl: (url, init) => fetch(url, { ...init, signal: AbortSignal.timeout(2000) }),
  ```

- Before the first load the core is empty: `isEnabled()` returns `false`,
  `getVariant()` `null`. Call `ensureFresh()` first.
- Each refresh replaces the whole flag set in the core.

## Security considerations

- Keep the SDK key in a secret binding or environment variable (for example
  `wrangler secret put CHECKGATE_SDK_KEY`). Checkgate SDK keys are public by
  design and limited to their environment's SDK endpoints (stream, snapshot,
  flag reads, impressions and events; everything else is 403), but there is no
  reason to expose one you only use server-side.
- The snapshot contains every flag of the environment, including targeting rule
  values and segment contents. It stays in memory unless you return it (for
  example through `snapshotFlags()` into an HTML payload).
- User attributes passed to evaluation are not sent anywhere.

## Compatibility

- ES module, no dependencies, no TypeScript declarations; `engines`:
  Node.js >= 18. Needs a global `fetch` or `fetchImpl`.
- Tested in CI: the unit suite (caching, stale-while-revalidate, fail-open,
  de-duplication) on Node.js 22 with a fake core.
- The Cloudflare Workers example is provided but not run in CI. Other edge
  runtimes (Deno Deploy, Fastly, Vercel Edge) need only `fetch` and a way to load
  the `.wasm` for `createWasmCore()`, and are untested.
- Works with servers older than 0.1.31: it requests the snapshot without
  `?v=2`, which every server answers with a plain array.

## Documentation

- [Edge evaluation guide](https://checkgate-sh.github.io/checkgate/ecosystem/edge)
- [SSR and bootstrap](https://checkgate-sh.github.io/checkgate/ecosystem/ssr) (`@checkgate/ssr`)
- [Web SDK and the shared engine](https://checkgate-sh.github.io/checkgate/sdks/web) (`@checkgate/web`)
- [Flags snapshot endpoint](https://checkgate-sh.github.io/checkgate/api-reference#flags-snapshot-poll-fallback)
- [Fly.io multi-region server example](https://github.com/checkgate-sh/checkgate/tree/main/edge/examples/fly)
- [Changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md)

## License

Apache-2.0
