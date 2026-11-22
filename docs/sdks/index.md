---
title: "Which Checkgate SDK do I need? — Next.js, React, Vue, React Native, Flutter, Node"
description: "Pick the right Checkgate packages for your stack. Required and optional SDKs for Next.js and SSR frameworks, Vite/Vue/React SPAs, React Native and Flutter mobile apps, Node.js servers, and edge runtimes."
---

# Which SDK do I need?

Every Checkgate SDK wraps the **same shared Rust evaluation core**, so a flag
resolves identically on a server, in a browser, and on a phone. What differs is
only *how the core is embedded* (NAPI, WebAssembly, JSI, FFI, PyO3) and *where the flag
snapshot comes from*.

Find your stack below — each section lists what's **required**, what's
**optional**, and why.

## At a glance

| Your stack | Required | Optional |
|---|---|---|
| **Next.js / Remix / SvelteKit / Nuxt** (server-rendered) | `@checkgate/web` + `@checkgate/ssr` | `@checkgate/edge`, `@checkgate/cli` |
| **Next.js — server-only flags** (no client usage) | `@checkgate/node` | `@checkgate/cli` |
| **SPA** — Vite, Vue, React, Svelte, Angular | `@checkgate/web` | `@checkgate/cli` |
| **React Native** | `@checkgate/react-native` | `@checkgate/cli` |
| **Flutter** | `checkgate_flutter` | `@checkgate/cli` |
| **Node.js server** — Express, Fastify, NestJS | `@checkgate/node` | `@checkgate/node-integrations`, `@checkgate/cli` |
| **Python server** — FastAPI, Django, Flask | `checkgate` (PyPI) | — |
| **Edge** — Cloudflare Workers, Vercel Edge, middleware | `@checkgate/edge` + `@checkgate/web` | `@checkgate/cli` |

`@checkgate/cli` is optional everywhere — it's a dev-time tool that generates
type-safe flag accessors. It never ships in your runtime bundle.

## Status by package

What you can install today, and the limits worth knowing before you pick. This table is
generated from the site's release data.

<SdkMatrix />

---

## Web frameworks with a server (Next.js, Remix, SvelteKit, Nuxt)

There are two valid setups. Pick based on **where you read flags**.

### A. Flags used in the browser → zero-flicker SSR

```bash
npm install @checkgate/web @checkgate/ssr
```

This is the setup most apps want. Without it, a client-side SDK shows default
values until its stream connects, so the "old" UI paints first and snaps to the
"new" one a moment later.

- **`@checkgate/web`** *(required)* — the live client SDK. It also exports the
  shared WASM engine at `@checkgate/web/core`, which resolves flags **on the
  server**. On Node it loads the engine from disk, so `await createWasmCore()`
  takes no arguments.
- **`@checkgate/ssr`** *(required)* — the glue. `buildBootstrap()` resolves flags
  server-side, `bootstrapScriptTag()` embeds them in the document, and
  `readBootstrap()` + `BootstrapValues` give the first client render the *exact*
  values the server used — so React hydration matches and nothing flickers. It
  has zero dependencies and ships no engine of its own.
- **`@checkgate/edge`** *(optional)* — adds TTL-cached snapshot fetching so you
  aren't re-fetching `/flags/snapshot` on every request. Skip it and fetch the
  snapshot yourself, passing it straight to `buildBootstrap({ snapshot })`.

See the [SSR / Bootstrap guide](/ecosystem/ssr) for the full three-step flow.

### B. Flags only read on the server

```bash
npm install @checkgate/node
```

If flags never reach the browser — server components, route handlers, API
routes — the Node SDK alone is enough. It keeps a live SSE connection and
`isEnabled()` is a synchronous in-memory call, so there's no per-request network
round-trip and nothing to hydrate.

> [!TIP]
> `@checkgate/node` is a native addon and runs in the **Node.js runtime only**.
> For Next.js middleware or any route set to the edge runtime, use
> `@checkgate/edge` + `@checkgate/web/core` instead.

---

## SPA — Vite, Vue, React, Svelte, Angular

```bash
npm install @checkgate/web
```

**One package, nothing else.** A pure SPA has no server render, so there's no
bootstrap to embed and nothing to hydrate — `@checkgate/ssr` would do nothing
for you.

`@checkgate/web` compiles the evaluation core to WebAssembly and runs it in the
browser: it streams flag updates over SSE, evaluates locally at near-native
speed, and falls back to polling `/flags/snapshot` if the stream can't reconnect.

```ts
import { CheckgateWeb } from '@checkgate/web'

const client = new CheckgateWeb({
  serverUrl: import.meta.env.VITE_CHECKGATE_URL,
  sdkKey: import.meta.env.VITE_CHECKGATE_SDK_KEY,
})
await client.connect()

client.isEnabled('new-homepage', userId)
```

See the [Web SDK reference](/sdks/web) for the React and Vue examples.

---

## Mobile

### React Native

```bash
npm install @checkgate/react-native
```

**One package.** The Rust core is embedded through **JSI**, so evaluation is a
direct synchronous call into native code with no bridge hop. The Rust engine is
compiled inside your app's Xcode / Gradle build, so the build machine needs a
Rust toolchain (and `cargo-ndk` for Android). Android's native build is verified
in CI; iOS is not certified for 0.1.31.

See the [React Native SDK reference](/sdks/react-native).

### Flutter

```bash
flutter pub add checkgate_flutter
```

**One package**, bound to the same core through **Dart FFI**. No compiled engine
is bundled: you build the Rust engine and link it into your app (verified on the
iOS simulator for 0.1.31). Note this is a pub.dev package — the `@checkgate/*`
npm packages are not involved.

See the [Flutter SDK reference](/sdks/flutter).

### Python <Since v="0.2.0" />

**One package**, `checkgate`: the Python client with the same Rust core compiled
in through PyO3 (abi3, CPython 3.11+): `pip install checkgate`
([installation](/sdks/python#installation)). Evaluation is synchronous and
thread-safe; syncing runs on a background thread, so it works the same under
WSGI and ASGI servers. Create one client per worker process.

See the [Python SDK reference](/sdks/python).

---

## Node.js server (Express, Fastify, NestJS)

```bash
npm install @checkgate/node
```

**One package.** The core is embedded via **NAPI**, so `isEnabled()` is a
synchronous in-memory lookup — safe to call in a hot request path. The client
holds one SSE connection per process and updates its local store as flags
change, so a flag flip reaches every server in milliseconds without polling.

```js
const { CheckgateClient } = require('@checkgate/node')

const client = new CheckgateClient({
  serverUrl: process.env.CHECKGATE_URL,
  sdkKey: process.env.CHECKGATE_SDK_KEY,
})
await client.connect()

app.get('/', (req, res) => {
  if (client.isEnabled('new-homepage', req.user.id)) { /* … */ }
})
```

See the [Node.js SDK reference](/sdks/nodejs).

**Optional: `@checkgate/node-integrations`** <Since v="0.2.0" /> — middleware for Express, a plugin
for Fastify and a module for NestJS. Each request gets flags bound to its own
user (`req.checkgate.isEnabled('new-homepage')`), startup waits for flags with
a timeout instead of hanging, and shutdown disconnects the client. See
[Express, Fastify and NestJS](/sdks/node-frameworks).

---

## Edge runtimes (Cloudflare Workers, Vercel Edge, middleware)

```bash
npm install @checkgate/edge @checkgate/web
```

Edge isolates are short-lived and can't hold a long-running SSE connection, so
the edge client fetches a **snapshot** instead and caches it in memory with a TTL
plus stale-while-revalidate.

- **`@checkgate/edge`** *(required)* — snapshot fetching, caching, and refresh.
- **`@checkgate/web`** *(required)* — supplies the evaluation engine via
  `@checkgate/web/core`. In Workers, pass the wasm module explicitly, since
  Wrangler compiles a `.wasm` import into a `WebAssembly.Module`:

```js
import { createWasmCore } from '@checkgate/web/core'
import wasmModule from '@checkgate/web/dist/checkgate_bg.wasm'

const core = await createWasmCore(wasmModule)
```

See the [Edge evaluation guide](/ecosystem/edge).

---

## Optional everywhere: type-safe flag accessors

```bash
npm install -D @checkgate/cli
```

`@checkgate/cli` generates typed accessors from your live flags for TypeScript,
Rust, and Dart — so a renamed or deleted flag becomes a compile error instead of
a silent `false`. It's a **dev dependency** and never ships in your runtime
bundle. See the [CLI guide](/ecosystem/cli).

## Not evaluation SDKs

These manage flags rather than evaluate them, and are unrelated to your app's
runtime:

- **[Terraform / OpenTofu provider](/ecosystem/terraform)** — manage flags and
  segments as code.
- **[Kubernetes operator](/ecosystem/kubernetes)** — reconcile `FeatureFlag`
  custom resources into Checkgate.
- **`checkgate-go`** — the shared Go REST client backing both of the above; also
  usable directly for automation that creates or updates flags.
