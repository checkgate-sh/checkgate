# @checkgate/openfeature-provider

[OpenFeature](https://openfeature.dev) provider for Checkgate (Node.js, server
paradigm). Flags are evaluated locally by the Checkgate Rust engine in
[`@checkgate/node`](https://www.npmjs.com/package/@checkgate/node): no network
request per evaluation.

**Current release: 0.2.0**, the first release of this package (new in Checkgate
0.2.0). Changes are listed in the
[changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md).

## What it does

- Implements the OpenFeature server `Provider` interface on top of a
  `CheckgateClient`. Code written against the OpenFeature API keeps working.
- Answers every evaluation with the client's `getDetails()`: the same engine,
  the same results as calling `@checkgate/node` directly, with Checkgate's
  reason mapped to OpenFeature's.
- Maps the client's connection state onto the provider lifecycle (`READY`,
  `STALE`, `ERROR`, `FATAL`) and flag changes onto
  `PROVIDER_CONFIGURATION_CHANGED`.
- Forwards OpenFeature `track()` calls as Checkgate goal events.

## Installation

```bash
npm install @checkgate/openfeature-provider @checkgate/node @openfeature/server-sdk
```

`@checkgate/node` (the engine, `>=0.2.0 <1`) and `@openfeature/server-sdk`
(`^1.17.0`) are peer dependencies, so your application installs them once.

## Minimal example

```typescript
import { OpenFeature } from '@openfeature/server-sdk'
import { CheckgateProvider } from '@checkgate/openfeature-provider'

const provider = new CheckgateProvider({
  serverUrl: process.env.CHECKGATE_URL!, // e.g. http://localhost:3000
  sdkKey: process.env.CHECKGATE_SDK_KEY!,
  initTimeoutMs: 5000,
})

await OpenFeature.setProviderAndWait(provider) // resolves when the first flag set has arrived
const client = OpenFeature.getClient()

const context = { targetingKey: 'user-123', plan: 'pro' }
const enabled = await client.getBooleanValue('new-checkout', false, context)
const theme = await client.getStringDetails('checkout-theme', 'light', context)
console.log(enabled, theme.value, theme.reason, theme.flagMetadata)

await OpenFeature.close() // on shutdown: disconnects the client the provider created
```

Register the provider once per process, at startup. Use
`OpenFeature.setProviderAndWait(domain, provider)` to bind it to a domain
instead of the default provider.

To use the same connection directly as well, create the client yourself and
pass it in; the provider then never disconnects it:

```typescript
import { OpenFeature } from '@openfeature/server-sdk'
import { CheckgateClient } from '@checkgate/node'
import { CheckgateProvider } from '@checkgate/openfeature-provider'

const checkgate = new CheckgateClient({ serverUrl: process.env.CHECKGATE_URL!, sdkKey: process.env.CHECKGATE_SDK_KEY })
await OpenFeature.setProviderAndWait(new CheckgateProvider({ client: checkgate }))

checkgate.isEnabled('new-checkout', 'user-123', { plan: 'pro' }) // same flags, direct call
```

## Configuration

| Option | Default | Description |
|---|---|---|
| `client` | none | An existing `CheckgateClient`. The provider connects it if needed and never disconnects it. |
| `serverUrl`, `sdkKey`, ... | none | Without `client`: any [`CheckgateClient` option](https://checkgate-sh.github.io/checkgate/sdks/nodejs#new-checkgateclient-options); the provider creates, connects and on close disconnects its own client. `serverUrl` is then required. |
| `initTimeoutMs` | no limit | Stop waiting for the first flag set after this many milliseconds: `initialize()` fails (provider `ERROR`), the client keeps connecting in the background, and the provider turns `READY` when it succeeds. |
| `requireTargetingKey` | `true` | Resolve `TARGETING_KEY_MISSING` when the context has no `targetingKey`. With `false`, a missing key is evaluated as the empty string (fine for flags without percentage rollouts or weighted variants). |

## Evaluation context

| OpenFeature context field | Checkgate |
|---|---|
| `targetingKey` | the user key (percentage rollouts and weighted variants hash it) |
| string | attribute, unchanged |
| finite number, boolean | attribute, converted to a string (`30` becomes `"30"`; numeric operators still compare numerically) |
| `Date` | attribute, as an ISO-8601 string |
| object, array, `null`, `NaN`, `Infinity` | left out |

The mapping is exported as `toAttributes(context)`.

## Value types

| OpenFeature call | Checkgate flag type |
|---|---|
| `getBooleanValue` / `getBooleanDetails` | boolean |
| `getStringValue` / `getStringDetails` | string |
| `getNumberValue` / `getNumberDetails` | integer |
| `getObjectValue` / `getObjectDetails` | JSON (object or array) |

A flag whose value has another type resolves `TYPE_MISMATCH`. A flag that is
off (or on with no value configured) and has no value of the requested type
returns your default without an error. `variant` is the resolved value as a
string for boolean, string and number flags.

## Reasons and errors

| Checkgate result | OpenFeature `reason` |
|---|---|
| `DISABLED` (flag switched off) | `DISABLED` |
| `PREREQUISITE_FAILED` | `PREREQUISITE_FAILED` (custom reason) |
| `TARGETING_MATCH` (a rule matched) | `TARGETING_MATCH` |
| `SPLIT` and `ROLLOUT_EXCLUDED` (percentage rollout or weighted variant, in or out) | `SPLIT` |
| `DEFAULT` (rules present, none matched) | `DEFAULT` |
| `STATIC` (no rules, rollout or variants) | `STATIC` |

`flagMetadata.checkgateReason` carries the exact Checkgate reason, plus
`ruleIndex`, `variantIndex` and `rolloutBucket` when they apply. The mapping is
exported as `REASONS`.

On an error, OpenFeature returns your default value with `reason: 'ERROR'` and
one of these `errorCode`s:

| `errorCode` | When |
|---|---|
| `FLAG_NOT_FOUND` | The flag does not exist in the environment. |
| `TYPE_MISMATCH` | The flag resolved to a value of another type (see above). |
| `TARGETING_KEY_MISSING` | No `targetingKey` in the context and `requireTargetingKey` is `true`. |
| `PROVIDER_NOT_READY` | The provider has not finished initializing (also after an `initTimeoutMs` timeout, until the first flag set arrives). |
| `PROVIDER_FATAL` | The SDK key was rejected before the first flag set. |

## Provider lifecycle

| Checkgate client | Provider status / event |
|---|---|
| first flag set received | `READY` |
| stream lost, reconnecting (last known flags still served) | `STALE` |
| poll fallback with polls failing (flags still served) | `STALE` |
| stream back, or polls succeeding again | `READY` |
| `initTimeoutMs` elapsed before the first flag set | `ERROR`, then `READY` when it arrives |
| `initTimeoutMs` elapsed, flags loaded from the client's `storage` cache | `READY`, then `STALE` until the server is reached |
| SDK key rejected before the first flag set | `FATAL` (`setProviderAndWait` rejects) |
| SDK key rejected later | `ERROR` (held flags keep being served) |

A flag change emits `PROVIDER_CONFIGURATION_CHANGED` with `flagsChanged`; the
flags of one snapshot are reported in one event. `OpenFeature.close()` calls
the provider's `onClose()`, which unsubscribes from the client and disconnects
it only if the provider created it.

## Tracking

`client.track('checkout', { targetingKey: 'user-123' }, { value: 49.99 })`
records a Checkgate goal event for experiments: a numeric `value` becomes the
event value and the remaining details its context. Calls without a
`targetingKey` are ignored.

## Local evaluation

Evaluations never reach the network: the client holds the flags in memory,
kept current by one stream (or poll fallback) per client. While disconnected,
evaluations keep answering from the last known flags and the provider reports
`STALE`. Impressions are reported in the background as configured on the
client (`reportImpressions`, `sendEvaluationContext`).

## Compatibility

- Node.js: the package declares `engines: { node: ">=20" }`; CI tests it on
  Node.js 22, 24 and 26 (Node.js 20 reached end of life on 2026-04-30).
- `@openfeature/server-sdk` `^1.17.0` (CI tests 1.23.0), server paradigm only.
- `@checkgate/node` `>=0.2.0 <1`, which also sets the supported platforms
  (prebuilt native binaries for Linux, macOS and Windows; see its README).
- CommonJS package with TypeScript declarations; `import` from ES modules works.

## Security considerations

- The SDK key and `serverUrl` are passed to `@checkgate/node`; keep the key in
  configuration or a secret store and use an `https://` server URL outside
  local development.
- Context attributes are used for local evaluation only. They leave the process
  only if the client is created with `sendEvaluationContext: true`; the
  `targetingKey` is sent as the user id of impressions and tracked events.
- Error messages from a rejected SDK key do not contain the key.

## Documentation

- [OpenFeature provider guide](https://checkgate-sh.github.io/checkgate/sdks/openfeature)
- [Node.js SDK](https://checkgate-sh.github.io/checkgate/sdks/nodejs) (client options, `getDetails()`)
- [SDK diagnostics](https://checkgate-sh.github.io/checkgate/operations/sdk-diagnostics)
- [Changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md)
- [Source and issues](https://github.com/checkgate-sh/checkgate)

## Development

From `sdks/openfeature-provider` in a checkout of the repository:

```bash
(cd ../nodejs && npm ci && npm run build)   # the native engine
npm ci && npm run typecheck && npm test
node bench/overhead.mjs                        # provider overhead vs. direct SDK calls
```

## License

Apache-2.0
