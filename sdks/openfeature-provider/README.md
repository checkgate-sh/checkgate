# @checkgate/openfeature-provider

[OpenFeature](https://openfeature.dev) provider for [Checkgate](https://github.com/checkgate-sh/checkgate)
(Node.js, server paradigm). Flags are evaluated in-process by the Checkgate Rust engine that
powers [`@checkgate/node`](https://www.npmjs.com/package/@checkgate/node): no network request
per evaluation, the same results as calling the SDK directly.

```bash
npm install @checkgate/openfeature-provider @checkgate/node @openfeature/server-sdk
```

```typescript
import { OpenFeature } from '@openfeature/server-sdk'
import { CheckgateProvider } from '@checkgate/openfeature-provider'

const provider = new CheckgateProvider({
  serverUrl: process.env.CHECKGATE_URL!,
  sdkKey: process.env.CHECKGATE_SDK_KEY!,
})

await OpenFeature.setProviderAndWait(provider)
const client = OpenFeature.getClient()

const enabled = await client.getBooleanValue('new-checkout', false, {
  targetingKey: 'user-123',
  plan: 'pro',
})
```

- `targetingKey` is the user key; other context fields are targeting attributes (numbers,
  booleans and dates are converted to strings; objects and arrays are left out).
- Reasons: `DISABLED`, `TARGETING_MATCH`, `SPLIT`, `DEFAULT`, `STATIC`, and the custom
  `PREREQUISITE_FAILED`. `flagMetadata` carries `checkgateReason`, `ruleIndex`,
  `variantIndex` and `rolloutBucket` when they apply.
- Lifecycle: `READY` after the first flag set, `STALE` while reconnecting (last known flags
  are still served), `FATAL` if the SDK key is rejected at startup. `initTimeoutMs` bounds the
  startup wait.
- `new CheckgateProvider({ client })` shares an existing `CheckgateClient`; the provider never
  disconnects a client it did not create.

Full documentation: [OpenFeature provider](https://checkgate-sh.github.io/checkgate/sdks/openfeature).

## Development

```bash
(cd ../nodejs && npm ci && npm run build)   # the native engine
npm ci && npm run typecheck && npm test
node test/bench.mjs                          # provider overhead vs. direct SDK calls
```

Apache-2.0
