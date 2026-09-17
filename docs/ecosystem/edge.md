---
title: "Edge Evaluation — Feature flags on Cloudflare, Fly & any edge runtime"
description: "Evaluate Checkgate feature flags at the edge with @checkgate/edge: a runtime-agnostic evaluator with TTL caching, stale-while-revalidate, and fail-open resilience."
---

# Edge (`@checkgate/edge`)

`@checkgate/edge` is a tiny, **runtime-agnostic** edge evaluator for Checkgate — it runs anywhere `fetch` exists: Cloudflare Workers, Deno Deploy, Fastly Compute, Vercel Edge, and Fly.io.

It owns the edge concern — pull a flag snapshot from your Checkgate server, cache it with a TTL and optional stale-while-revalidate, and survive origin outages by keeping the last-known-good snapshot — and delegates the actual evaluation to an injected engine. In production that engine is the shared [`@checkgate/web`](/sdks/web) WebAssembly core, so edge evaluation is **identical to every other Checkgate SDK**. There is no re-implemented rule / rollout / segment / prerequisite logic to drift.

Zero dependencies.

## Install

```bash
npm install @checkgate/edge
```

## Usage

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

- **Fail-open.** If a refresh can't reach origin, the previous snapshot keeps serving — an origin blip never takes your flags down. The *first* load has nothing to fall back to, so it surfaces the error.
- **De-duplicated refresh.** A burst of concurrent `ensureFresh()` calls triggers at most one snapshot fetch.
- **Injected `core`, `fetch`, and clock.** That's what makes the package dependency-free and fully unit-testable in plain Node — no WASM build or edge runtime is required to verify the caching and resilience logic.

## Example: Cloudflare Workers

Evaluate flags in the same isolate that serves the request, across Cloudflare's 300+ locations, with no round-trip to origin on the hot path.

```
                 cron (every 60s)            per request (warm isolate)
  Checkgate  ──────────────────▶  Worker  ─────────────────────────▶  in-memory
   /flags/snapshot   refresh()    isolate      isEnabled()/getValue()     WASM eval
```

- The snapshot + WASM engine live in a **module-global** that Cloudflare keeps alive across requests on a warm isolate, so most requests never touch the network.
- `ensureFresh()` re-fetches only after the TTL; stale-while-revalidate refreshes in the background via `ctx.waitUntil` and never blocks a response.
- A **Cron Trigger** refreshes proactively so even cold isolates find a warm snapshot.

Setup:

```bash
npm install
# Store the SDK key as a secret (don't commit it):
npx wrangler secret put CHECKGATE_SDK_KEY
# Point at your server + tune caching in wrangler.toml ([vars]).
npm run dev      # local: http://localhost:8787/?uid=user_123
npm run deploy   # ship it
```

| Variable | Where | Purpose |
|---|---|---|
| `CHECKGATE_URL` | `wrangler.toml [vars]` | Checkgate server base URL |
| `CHECKGATE_SDK_KEY` | `wrangler secret` | Environment-scoped SDK key (Bearer) |
| `CHECKGATE_TTL_SECONDS` | `[vars]` | Freshness window before a re-fetch (default 30) |
| `CHECKGATE_SWR_SECONDS` | `[vars]` | Extra window to serve stale while refreshing (default 60) |

Pass a **stable per-user key** (cookie/header/token) so sticky, hash-based rollouts bucket each user consistently at the edge. Other edge runtimes work the same way — `@checkgate/edge` only needs `fetch`; swap the WASM adapter for that runtime's WASM loader.

## Example: Fly.io (multi-region)

On Fly there are two patterns:

1. **Run the full Checkgate server multi-region.** Fly places the server (Rust + WASM local evaluation) as Machines in many regions, so every SDK's SSE bootstrap, `/flags/snapshot` poll, and impression ingest is served from a nearby region — no app changes, and evaluation is still local inside each SDK.
2. **Run a Fly app *as* an edge evaluator** using `@checkgate/edge` (the same pattern as the Cloudflare example) for per-request edge evaluation inside your own Fly app.

For the server multi-region recipe, deploy from the repository root and wire up secrets:

```bash
fly launch --no-deploy --copy-config --config edge/examples/fly/fly.toml
fly secrets set \
  DATABASE_URL="postgres://…" \
  REDIS_URL="redis://…" \
  SESSION_SECRET="$(openssl rand -hex 32)"
fly deploy --config edge/examples/fly/fly.toml

# Scale out across regions:
fly scale count 3 --region iad,fra,syd
```

Keep Postgres near your `primary_region` (or use read replicas), and point every region at the same Redis so flag changes propagate to all connected SDKs via SSE fan-out. See [Self-Hosting](/self-hosting) for the full environment-variable reference.

## See also

- [Core Concepts](/guide/concepts) — evaluation, rollouts, and the `/flags/snapshot` and SSE mechanics.
- [Web SDK](/sdks/web) — the shared WASM engine behind edge evaluation.
- Source: [`edge/`](https://github.com/checkgate-sh/checkgate/tree/main/edge)
