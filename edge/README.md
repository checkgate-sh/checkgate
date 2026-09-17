<div align="center">
  <img src="../assets/logo.svg" alt="Checkgate" width="64" />
  <h1>@checkgate/edge</h1>
  <p>Evaluate Checkgate feature flags <strong>at the edge</strong> — Cloudflare Workers, Deno Deploy, Fastly, Vercel Edge, Fly.io.</p>
</div>

A tiny, **runtime-agnostic** edge evaluator. It owns the edge concern — pull a
flag snapshot from your Checkgate server, cache it with a TTL and optional
stale-while-revalidate, survive origin outages by keeping the last-known-good
snapshot — and delegates the actual evaluation to an injected engine. In
production that engine is the shared `@checkgate/web` WebAssembly core, so edge
evaluation is **identical to every other Checkgate SDK**. No re-implemented
rule / rollout / segment / prerequisite logic to drift.

Zero dependencies. Works anywhere `fetch` exists.

## Install

```bash
npm install @checkgate/edge
```

## Use

```javascript
import { CheckgateEdge } from '@checkgate/edge'
import { createWasmCore } from '@checkgate/web/core'
// Wrangler compiles a .wasm import into a WebAssembly.Module. On Node (Next.js
// server) and in the browser, call createWasmCore() with no argument instead.
import wasmModule from '@checkgate/web/dist/checkgate_bg.wasm'

const edge = new CheckgateEdge({
  serverUrl: 'https://flags.your-domain.com',
  sdkKey: env.CHECKGATE_SDK_KEY,          // environment-scoped SDK key (Bearer)
  core: await createWasmCore(wasmModule), // the WASM evaluation engine
  ttlSeconds: 30,                         // re-fetch the snapshot at most this often
  staleWhileRevalidateSeconds: 60,        // serve stale + refresh in background
})

// On each request (fast: in-memory on a warm isolate):
await edge.ensureFresh({ waitUntil: (p) => ctx.waitUntil(p) })
const showIt = edge.isEnabled('new-homepage', userKey)
const color  = edge.getValue('checkout-color', userKey, {}, 'blue')
```

## API

| Member | Description |
|---|---|
| `new CheckgateEdge({ serverUrl, sdkKey, core, ttlSeconds?, staleWhileRevalidateSeconds?, fetchImpl?, now?, snapshotPath? })` | Construct. `core` is the injected engine. |
| `refresh()` | Fetch the snapshot and load it into the core. Concurrent calls are de-duplicated. Fails open to last-known-good. |
| `ensureFresh({ waitUntil? })` | Load if never loaded; serve from cache within the TTL; serve stale + background-refresh within the SWR window; block-refresh beyond it. |
| `isEnabled(key, userKey, attrs?)` / `getValue(key, userKey, attrs?, default?)` / `getVariant(key, userKey, attrs?)` | Delegate to the core. |
| `isLoaded` / `flagCount` / `lastLoadedAt` / `ageMs()` | Introspection. |

## Design

- **Fail-open.** If a refresh can't reach origin, the previous snapshot keeps
  serving — an origin blip never takes your flags down. The *first* load has
  nothing to fall back to, so it surfaces the error.
- **De-duplicated refresh.** A burst of concurrent `ensureFresh()` calls triggers
  at most one snapshot fetch.
- **Injected `core`, `fetch`, and clock.** That's what makes it dependency-free
  and fully unit-tested in plain Node (`npm test`) — no WASM build or edge runtime
  required to verify the caching/resilience logic.

## Examples

- [`examples/cloudflare-worker`](examples/cloudflare-worker) — per-request edge
  evaluation on Cloudflare Workers, with a Cron Trigger keeping the snapshot warm.
- [`examples/fly`](examples/fly) — deploy the full Checkgate server multi-region
  on Fly.io.

---

Part of [Checkgate](https://github.com/checkgate-sh/checkgate) — the self-hosted feature-flag platform.
