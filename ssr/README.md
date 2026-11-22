<p align="center">
  <img src="https://raw.githubusercontent.com/checkgate-sh/checkgate/main/assets/logo.svg" width="64" alt="Checkgate">
</p>

# @checkgate/ssr

Server-side rendering helpers for [Checkgate](https://github.com/checkgate-sh/checkgate),
the self-hosted feature-flag platform: evaluate flags on the server, embed the
result in the HTML, and start the browser SDK from it so the first client
render matches the server render.

**Current release: 0.2.0.** No API changes since 0.1.31; the package is
versioned with the platform release. See the
[changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md).

## What it does

Without a bootstrap, a browser SDK has no flags until its stream delivers them,
so the page first renders with defaults and then switches. With
`@checkgate/ssr`:

1. **Server:** `buildBootstrap()` loads a flag snapshot into an evaluation
   engine, resolves the flags the page needs for the current user, and returns a
   serializable payload. By default the payload also carries the snapshot
   itself.
2. **HTML:** `bootstrapScriptTag()` embeds the payload in one `<script>` tag,
   escaped so flag data cannot break out of it.
3. **Browser:** `readBootstrap()` and `BootstrapValues` give the server-resolved
   values for the first render; passing the payload as `bootstrap` to
   `CheckgateWeb` lets the live client evaluate at once and then follow the
   stream.

The package has no dependencies and no framework code. It evaluates nothing
itself: the engine is `@checkgate/web/core`, the same WebAssembly build of the
Rust engine the browser SDK uses, so server and client resolve flags the same
way.

## Installation

```bash
npm install @checkgate/ssr @checkgate/web
```

- `@checkgate/web` is required: `@checkgate/web/core` is the engine for
  `buildBootstrap()`, and `CheckgateWeb` is the live client.
- [`@checkgate/edge`](https://www.npmjs.com/package/@checkgate/edge) is optional:
  it fetches and caches the snapshot on the server (TTL, stale-while-revalidate,
  fail-open). Without it, fetch `GET /flags/snapshot` yourself.

## Minimal example

Server (any Node.js framework):

```js
import { buildBootstrap, bootstrapScriptTag } from '@checkgate/ssr'
import { createWasmCore } from '@checkgate/web/core'

const core = await createWasmCore()

export async function renderPage(userKey, attributes) {
  // GET /flags/snapshot without ?v=2 answers a plain array of flags.
  const res = await fetch(`${process.env.CHECKGATE_URL}/flags/snapshot`, {
    headers: { Authorization: `Bearer ${process.env.CHECKGATE_SDK_KEY}` },
  })
  if (!res.ok) throw new Error(`Checkgate snapshot: HTTP ${res.status}`)
  const snapshot = await res.json()

  const payload = buildBootstrap({ core, snapshot, userKey, attributes, keys: ['new-homepage'] })
  const body = payload.values['new-homepage']?.enabled ? '<h1>New home</h1>' : '<h1>Home</h1>'
  return `<!doctype html><html><head>${bootstrapScriptTag(payload)}</head>` +
    `<body>${body}<script type="module" src="/app.js"></script></body></html>`
}
```

Browser (`/app.js`):

```js
import { readBootstrap, BootstrapValues } from '@checkgate/ssr'
import { CheckgateWeb } from '@checkgate/web'

const payload = readBootstrap()
const initial = new BootstrapValues(payload)
initial.isEnabled('new-homepage') // the value the server rendered with

const flags = new CheckgateWeb({
  serverUrl: 'https://flags.example.com',
  sdkKey: 'sk_live_...',
  bootstrap: payload, // evaluates from the embedded snapshot, then follows the stream
})
await flags.connect()
flags.isEnabled('new-homepage', initial.userKey, attributes) // live; same user key and attributes as the server
```

Use the same user key and attributes on both sides, or the live client can
resolve differently from the server.

## Configuration

`buildBootstrap(options)`:

| Option | Type | Default | Description |
|---|---|---|---|
| `core` | object | required | Engine with `clear()`, `upsertFlag(json)`, `getVariant()`; normally `await createWasmCore()` from `@checkgate/web/core`. `buildBootstrap()` clears it and loads `snapshot`. |
| `snapshot` | `object[]` | required | Array of flags as `GET /flags/snapshot` returns them (or `CheckgateEdge#snapshotFlags()`). |
| `userKey` | `string` | required | Stable user identifier used for evaluation. |
| `keys` | `string[]` | all flags | Flag keys to resolve. Omitted or empty: every flag in the snapshot. |
| `attributes` | `object` | `{}` | Targeting attributes of the user. |
| `includeSnapshot` | `boolean` | `true` | Embed the snapshot as `flags`, so `CheckgateWeb` can evaluate before its stream is up. |
| `now` | `() => number` | `Date.now` | Clock for `generatedAt` (for tests). |

`bootstrapScriptTag(payload, { varName, nonce })`: `varName` defaults to
`__CHECKGATE_BOOTSTRAP__` (exported as `DEFAULT_VAR_NAME`); `nonce` adds a CSP
`nonce` attribute.

## API

| Export | Side | Description |
|---|---|---|
| `buildBootstrap(options)` | server | Returns `{ v, userKey, generatedAt, values, flags? }`; `values` maps each resolved key to `{ enabled, value }`. Keys not in the snapshot are left out. |
| `serializeBootstrap(payload)` | server | JSON with `<`, `>`, `&`, U+2028 and U+2029 escaped, safe inside `<script>`. |
| `bootstrapScriptTag(payload, opts?)` | server | `<script>window.__CHECKGATE_BOOTSTRAP__=...</script>`. |
| `readBootstrap(varName?, win?)` | client | The embedded payload, or `null` if absent (`win` defaults to `globalThis`). |
| `new BootstrapValues(payload)` | client | `.isEnabled(key)`, `.getValue(key, defaultValue)`, `.getVariant(key)`, `.has(key)`, plus `.userKey`, `.flags`, `.valid`. |
| `BOOTSTRAP_VERSION`, `DEFAULT_VAR_NAME` | both | Payload schema version (`1`) and the default global name. |

## The flow (Next.js App Router)

The server builds the payload and passes it to a client provider as a prop, so
the server render and the first client render read the same values.

```js
// lib/checkgate-server.js
import { CheckgateEdge } from '@checkgate/edge'
import { buildBootstrap } from '@checkgate/ssr'
import { createWasmCore } from '@checkgate/web/core'

let edge = null

export async function getBootstrap(userKey, attributes, keys) {
  edge ??= new CheckgateEdge({
    serverUrl: process.env.CHECKGATE_URL,
    sdkKey: process.env.CHECKGATE_SDK_KEY,
    core: await createWasmCore(),
    ttlSeconds: 30,
  })
  await edge.ensureFresh()
  return buildBootstrap({ core: await createWasmCore(), snapshot: edge.snapshotFlags(), userKey, attributes, keys })
}
```

```jsx
// app/layout.jsx (Server Component)
import { getBootstrap } from '../lib/checkgate-server'
import { FlagsProvider } from './flags-provider'

export default async function RootLayout({ children }) {
  const userKey = 'anonymous'          // from your session or cookie
  const attributes = { plan: 'free' }  // the same attributes for server and client
  const payload = await getBootstrap(userKey, attributes, ['new-homepage', 'checkout-color'])
  return (
    <html>
      <body>
        <FlagsProvider payload={payload} attributes={attributes}>{children}</FlagsProvider>
      </body>
    </html>
  )
}
```

```jsx
// app/flags-provider.jsx
'use client'
import { createContext, useContext, useEffect, useMemo, useState } from 'react'
import { BootstrapValues } from '@checkgate/ssr'
import { CheckgateWeb } from '@checkgate/web'

const FlagsContext = createContext(() => false)

export function FlagsProvider({ payload, attributes = {}, children }) {
  const initial = useMemo(() => new BootstrapValues(payload), [payload])
  const [client, setClient] = useState(null)
  const [, setVersion] = useState(0)

  useEffect(() => {
    const live = new CheckgateWeb({
      serverUrl: process.env.NEXT_PUBLIC_CHECKGATE_URL,
      sdkKey: process.env.NEXT_PUBLIC_CHECKGATE_SDK_KEY,
      bootstrap: payload,
    })
    const off = live.onChange(() => setVersion((v) => v + 1))
    live.connect().then(() => setClient(live)).catch(() => {})
    return () => { off(); live.disconnect() }
  }, [payload])

  // Server-resolved values until the live client is ready, then live ones.
  const read = (key) =>
    client && initial.userKey ? client.isEnabled(key, initial.userKey, attributes) : initial.isEnabled(key)
  return <FlagsContext.Provider value={read}>{children}</FlagsContext.Provider>
}

export const useFlag = (key) => useContext(FlagsContext)(key)
```

Outside React, embed with `bootstrapScriptTag(payload)` and read with
`readBootstrap()` as in the minimal example. Remix and SvelteKit follow the
same steps: build the payload in the loader or `+page.server`, embed it in the
document, read it on the client.

## Errors and lifecycle

- `buildBootstrap()` throws if `core` lacks the required methods, `snapshot` is
  not an array (for example the `?v=2` object form), or `userKey` is empty.
- A requested key that is not in the snapshot is absent from `values`;
  `BootstrapValues` then returns `false`, the default value or `null`.
- `BootstrapValues` never throws: a missing payload or one with another schema
  version gives `valid: false` and defaults everywhere.
- The payload is as fresh as the snapshot you pass (with `@checkgate/edge`, up
  to the TTL plus stale window). `CheckgateWeb` replaces the embedded flags with
  the server's current set when its stream delivers it; no `onChange` fires for
  that first set, so re-render when `connect()` resolves (as the provider above
  does).
- `buildBootstrap()` clears and reloads the `core` on every call, synchronously,
  so one core can be reused across requests.

## Security considerations

- With `includeSnapshot: true` (the default) the HTML contains **every flag of
  the environment**, including targeting rule values and segment contents. That
  is the same data the browser SDK downloads with its public SDK key, but it is
  now also in your HTML. Use `includeSnapshot: false` to embed only the resolved
  values of the requested `keys` (the live client then starts from its stored
  flags, if any, and its stream).
- The payload contains the `userKey` and is specific to one user: do not let a
  shared cache serve that HTML to other users.
- `serializeBootstrap()` escapes `<`, `>`, `&`, U+2028 and U+2029, so a flag key
  or value containing `</script>` cannot end the tag. The `nonce` is inserted as
  given: pass only a nonce your server generated.
- The SDK key used on the server is the same kind of key the browser uses:
  public by design and limited to its environment's SDK endpoints (stream,
  snapshot, flag reads, impressions and events). Keep it in server-side
  configuration anyway.

## Compatibility

- ES module, no dependencies, no TypeScript declarations (the examples are
  JavaScript); `engines`: Node.js >= 18. The server helpers run
  wherever the engine (`@checkgate/web/core`) runs; the client helpers are
  plain JavaScript for the browser.
- Tested in CI: the unit suite (payload building, escaping, script tag,
  round-trip, schema checks) on Node.js 22 with a fake engine. The Next.js,
  Remix and SvelteKit flows are documentation and are not run in CI.

## Documentation

- [SSR and bootstrap guide](https://checkgate-sh.github.io/checkgate/ecosystem/ssr)
- [Web SDK](https://checkgate-sh.github.io/checkgate/sdks/web) (`@checkgate/web`, the `bootstrap` option)
- [Edge evaluation](https://checkgate-sh.github.io/checkgate/ecosystem/edge) (`@checkgate/edge`)
- [Flags snapshot endpoint](https://checkgate-sh.github.io/checkgate/api-reference#flags-snapshot-poll-fallback)
- [Changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md)

## License

Apache-2.0
