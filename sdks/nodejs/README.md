<p align="center">
  <img src="../../assets/logo.svg" width="64" alt="Checkgate">
</p>

# @checkgate/node

**Checkgate Node.js SDK (NAPI)** — The official high-performance, native Node.js client for Checkgate.
This SDK is built on top of the Rust native codebase, powered by NAPI-RS for absolute zero-overhead sub-microsecond feature flag evaluation directly in your server.

## Installation

```bash
npm install @checkgate/node
# or
pnpm add @checkgate/node
# or
yarn add @checkgate/node
```

## Quick Start

Initialize the Checkgate client with your unique server URL and SDK key:

```typescript
import { CheckgateClient } from '@checkgate/node'

const checkgate = new CheckgateClient({
  serverUrl: 'http://localhost:3000', // Your self-hosted Checkgate server URL
  sdkKey: 'sk_prod_xxxxxxxx',         // Your secure server-side SDK Key
})

async function run() {
  // Connect via SSE to instantly stream flag rules
  await checkgate.connect()

  // Evaluate features with sub-microsecond latency locally
  const showFeature = checkgate.isEnabled('new-dashboard', 'user_123', { 
    email: 'user@example.com' 
  })

  if (showFeature) {
    console.log("Welcome to the new dashboard!")
  }
}

run()
```

## A/B testing: track conversions

Record goal events with `track()` to measure how each flag variant converts. Pair
a flag with a goal event in an **Experiment** in the dashboard to see per-variant
conversion rates and statistical significance.

```typescript
const variant = checkgate.getVariant('checkout-button-color', 'user_123')

// …later, when the user converts:
checkgate.track('checkout_complete', 'user_123', { value: 49.99 })
```

Events are buffered and reported asynchronously (best-effort), just like impressions.
Use the **same `userKey`** you pass to `getVariant()`/`isEnabled()` so conversions
attribute to the right variant.

## Why Checkgate?
* **Sub-Microsecond Evaluation:** Flags are parsed instantly in-memory via Rust native bindings.
* **Instant Propagation:** Leverages SSE (Server Sent Events) to distribute flag toggles globally in < 50ms without polling.
* **Self-Hosted Privacy:** Keep your user data strictly within your own infrastructure bounds.

For more information and detailed architecture designs, check out the [official Checkgate documentation](https://checkgate-sh.github.io/checkgate).
