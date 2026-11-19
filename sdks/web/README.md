<p align="center">
  <img src="../../assets/logo.svg" width="64" alt="Checkgate">
</p>

# @checkgate/web

**Checkgate Web SDK (WebAssembly)** — The official frictionless frontend client for Checkgate.
This SDK leverages a lightning-fast Rust WebAssembly (WASM) payload to evaluate feature flags completely locally within the user's browser, bypassing standard frontend HTTP polling loops.

## Installation

```bash
npm install @checkgate/web
# or
pnpm add @checkgate/web
# or
yarn add @checkgate/web
```

## Quick Start

Create the client with your Checkgate server URL and an SDK key (SDK keys are
public by design; the stream sends it as a `?sdk_key=` query parameter):

```typescript
import { CheckgateWeb } from '@checkgate/web'

const checkgate = new CheckgateWeb({
  serverUrl: 'https://checkgate.your-company.com',
  sdkKey: 'sk_live_xxxxxxxx',
})

// Loads the WebAssembly core and the flag set over SSE.
await checkgate.connect()

// Evaluated in the browser: flag key, a stable user key, string attributes.
if (checkgate.isEnabled('beta-feature', 'user-12345', { plan: 'enterprise' })) {
  // Render the new beta UI…
}
```

TypeScript declarations are included. Until flags are loaded (by `connect()`,
the default `localStorage` cache, or a `bootstrap` payload from
`@checkgate/ssr`), `isEnabled()` returns `false`. Impressions (flag key, user
key, result) are reported by default; pass `reportImpressions: false` to turn
them off.

## Connection health

A stream that goes silent for 3 server heartbeats (45 s by default) is dropped
and reconnected. A rejected SDK key (HTTP 401/403) is not retried: `connect()`
rejects with `CheckgateAuthError` and the last known flags keep being served.
`client.getStatus()` returns `{ state, lastSyncAt, error }` so you can tell how
fresh the flags are. See the [SDK docs](https://checkgate-sh.github.io/checkgate/sdks/).

## A/B testing: track conversions

Record goal events with `track()` to measure how each flag variant converts. Pair
a flag with a goal event in an **Experiment** in the dashboard to see per-variant
conversion rates and statistical significance.

```javascript
const variant = checkgate.getVariant('checkout-button-color', 'user_123')

// …later, when the user converts:
checkgate.track('checkout_complete', 'user_123', { value: 49.99 })
```

Events are buffered and reported asynchronously (best-effort), just like impressions.
Use the **same `userKey`** you pass to `getVariant()`/`isEnabled()` so conversions
attribute to the right variant.

## Why Checkgate WebAssembly?
* **Synchronous evaluation:** once flags are loaded, every check is a local WASM call, with no request per decision. Seed it from `@checkgate/ssr` or the local cache to avoid a loading state on first paint.
* **Live SSE:** toggling a flag in the dashboard updates the page without a reload.
* **Open Source Alternative:** Stop paying premium per-seat pricing for basic feature gates.

Learn more natively in the [official Checkgate documentation](https://checkgate-sh.github.io/checkgate).
