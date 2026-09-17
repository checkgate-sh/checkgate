<div align="center">
  <img src="../assets/checkgate_logo.png" alt="Checkgate" width="72" />
  <h1>@checkgate/ssr</h1>
  <p>Server-render initial flag state and hydrate the client with <strong>zero flag flicker</strong> — Next.js, Remix, SvelteKit, or any SSR framework.</p>
</div>

Without SSR, a client-side flag SDK shows default values until its stream
bootstraps — so the "old" UI paints first and snaps to the "new" one a moment
later. `@checkgate/ssr` removes that flash:

1. **On the server**, evaluate the flags a page needs for the current user and
   build a tiny, serializable **bootstrap payload**.
2. **Embed** it in the HTML (a single `<script>` tag, XSS-safe).
3. **On the client**, render the first paint straight from that payload — byte
   for byte what the server rendered — then let the live SDK take over for
   updates.

Zero dependencies. Framework-agnostic. Runs anywhere `fetch` exists.

## Install

```bash
npm install @checkgate/ssr @checkgate/edge @checkgate/web
```

`@checkgate/ssr` builds and reads the payload; `@checkgate/edge` fetches the flag
snapshot on the server; `@checkgate/web` is the live client SDK (and supplies the
shared WASM evaluation engine both sides use, so server and client agree exactly).

## The flow (Next.js App Router)

### 1. Server — build the payload

```ts
// lib/checkgate-server.ts
import { CheckgateEdge } from '@checkgate/edge'
import { buildBootstrap } from '@checkgate/ssr'
import { createWasmCore } from '@checkgate/web/core'

let edge: CheckgateEdge | null = null

export async function getBootstrap(userKey: string, keys: string[]) {
  if (!edge) {
    edge = new CheckgateEdge({
      serverUrl: process.env.CHECKGATE_URL!,
      sdkKey: process.env.CHECKGATE_SDK_KEY!,
      core: await createWasmCore(),
      ttlSeconds: 30,
    })
  }
  await edge.ensureFresh()
  // buildBootstrap re-loads the snapshot into a core and resolves `keys` for the
  // user; pass the same snapshot the edge client is holding.
  return buildBootstrap({
    core: await createWasmCore(),
    snapshot: edge.snapshotFlags(), // your app can also fetch /flags/snapshot directly
    userKey,
    keys,
  })
}
```

### 2. Embed it in the document

```tsx
// app/layout.tsx
import { bootstrapScriptTag } from '@checkgate/ssr'
import { getBootstrap } from '@/lib/checkgate-server'

export default async function RootLayout({ children }) {
  const userKey = /* cookie/session id */ 'anonymous'
  const payload = await getBootstrap(userKey, ['new-homepage', 'checkout-color'])
  return (
    <html>
      <head dangerouslySetInnerHTML={{ __html: bootstrapScriptTag(payload) }} />
      <body>{children}</body>
    </html>
  )
}
```

### 3. Hydrate on the client

```tsx
'use client'
import { useEffect, useState } from 'react'
import { readBootstrap, BootstrapValues } from '@checkgate/ssr'
import { CheckgateWeb } from '@checkgate/web'

const bootstrap = readBootstrap()            // the embedded payload
const initial = new BootstrapValues(bootstrap)

export function useFlag(key: string) {
  // First paint uses the server-resolved value → no flicker.
  const [enabled, setEnabled] = useState(() => initial.isEnabled(key))

  useEffect(() => {
    // Seed the live client from the same snapshot, then subscribe to updates.
    const client = new CheckgateWeb({
      serverUrl: process.env.NEXT_PUBLIC_CHECKGATE_URL!,
      sdkKey: process.env.NEXT_PUBLIC_CHECKGATE_SDK_KEY!,
      bootstrap, // ← live-ready without waiting for SSE
    })
    client.connect().then(() => setEnabled(client.isEnabled(key, bootstrap.userKey)))
    return () => client.disconnect()
  }, [key])

  return enabled
}
```

**Remix / SvelteKit** follow the same three steps — build the payload in the
`loader` / `+page.server`, inject the script into the document template, and read
it in a client component/store.

## API

| Export | Side | Description |
|---|---|---|
| `buildBootstrap({ core, snapshot, userKey, keys?, attributes?, includeSnapshot? })` | server | Evaluate flags for a user, return a serializable payload. |
| `serializeBootstrap(payload)` | server | JSON, escaped for safe inlining in `<script>`. |
| `bootstrapScriptTag(payload, { varName?, nonce? })` | server | A ready-to-inject `<script>` tag (supports a CSP nonce). |
| `readBootstrap(varName?, win?)` | client | Read the embedded payload (or `null`). |
| `new BootstrapValues(payload)` | client | `.isEnabled(key)` / `.getValue(key, default)` / `.getVariant(key)` over the server-resolved values. |

## Safety

- The payload is serialized with `<`, `>`, `&`, and the U+2028/U+2029 line
  separators escaped, so a flag key or value containing `</script>` can't break
  out of the tag. `bootstrapScriptTag` accepts a `nonce` for strict CSPs.
- `BootstrapValues` is null-safe and version-checked: a missing or stale-schema
  payload degrades to defaults rather than throwing, so a bad embed never breaks
  a render.

---

Part of [Checkgate](https://github.com/checkgate-sh/checkgate) — the self-hosted feature-flag platform.
