---
title: "OpenFeature Provider for Node.js — Checkgate"
description: "Use Checkgate through the OpenFeature server SDK. Flags are evaluated locally by the Checkgate Rust engine: no network request per evaluation."
---

# OpenFeature provider (Node.js) <Since v="0.2.0" />

<ReleaseNotice pkg="openfeature" />

`@checkgate/openfeature-provider` plugs Checkgate into the
[OpenFeature](https://openfeature.dev) server SDK for Node.js. Code written against the
OpenFeature API keeps working; evaluations are answered by the same local engine as
[`@checkgate/node`](./nodejs), so there is no network request per evaluation.

## Installation

```bash
npm install @checkgate/openfeature-provider @checkgate/node @openfeature/server-sdk
```

Requires Node.js 22 or later (CI tests 22, 24 and 26), `@openfeature/server-sdk` 1.17 or later
(tested with 1.23) and `@checkgate/node` 0.2.

## Quick start

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

Create the provider once at startup, as you would a `CheckgateClient`. To use the same
connection directly as well, pass an existing client: `new CheckgateProvider({ client })`. The
provider then never disconnects it.

## Options

| Option | Default | |
|---|---|---|
| `client` | — | An existing `CheckgateClient` to use instead of creating one. |
| `serverUrl`, `sdkKey`, … | — | Any [`CheckgateClient` option](./nodejs#new-checkgateclient-options), when the provider creates its own client. |
| `initTimeoutMs` | wait | Stop waiting for the first flag set after this long: the provider reports `ERROR`, keeps connecting in the background, and turns `READY` when it succeeds. |
| `requireTargetingKey` | `true` | Without a `targetingKey`, resolve `TARGETING_KEY_MISSING`. With `false`, an empty key is used (fine for flags without percentage rollouts or weighted variants). |

## Evaluation context

`targetingKey` is the user key that percentage rollouts and weighted variants hash. Every other
context field becomes a targeting attribute. Checkgate attributes are strings, so numbers and
booleans are converted to strings (numeric operators still compare them as numbers), dates
become ISO-8601 strings, and objects, arrays and `null` are left out.

## Types

| OpenFeature call | Checkgate flag type |
|---|---|
| `getBooleanValue` | boolean |
| `getStringValue` | string |
| `getNumberValue` | integer (exact up to 2^53) |
| `getObjectValue` | JSON (object or array) |

A flag of another type resolves `TYPE_MISMATCH` and returns your default. A flag that is off, or
on with no value configured, and has no value of the requested type returns your default
without an error.

## Resolution details

| Checkgate | OpenFeature `reason` |
|---|---|
| flag switched off | `DISABLED` |
| a prerequisite is not satisfied | `PREREQUISITE_FAILED` (custom reason) |
| a targeting rule matched | `TARGETING_MATCH` |
| percentage rollout or weighted variant (in or out) | `SPLIT` |
| rules present, none matched | `DEFAULT` |
| no rules, rollout or variants | `STATIC` |
| flag does not exist | `ERROR`, `FLAG_NOT_FOUND` |

`variant` is the resolved value as a string for boolean, string and number flags.
`flagMetadata.checkgateReason` carries the exact Checkgate reason (`ROLLOUT_EXCLUDED` is
reported as `SPLIT` to OpenFeature), plus `ruleIndex`, `variantIndex` and `rolloutBucket`
when they apply. See [`getDetails()`](./nodejs#client-getdetails-flagkey-userkey-attributes-evaluationdetails-null).

## Provider lifecycle

| Checkgate client | Provider status |
|---|---|
| first flag set received | `READY` |
| stream lost, reconnecting (flags still served) | `STALE` |
| stream or poll back | `READY` |
| `initTimeoutMs` elapsed before the first flag set | `ERROR`, then `READY` when it arrives |
| SDK key rejected before the first flag set | `FATAL` |
| SDK key rejected later (held flags still served) | `ERROR` |

A flag change emits `PROVIDER_CONFIGURATION_CHANGED` with `flagsChanged`; the flags of one
snapshot are reported in one event. `OpenFeature.close()` disconnects a client the provider
created.

## Tracking

`client.track('checkout', { targetingKey: 'user-123' }, { value: 49.99 })` records a Checkgate
goal event for [experiments](../guide/experimentation), with the remaining details as its context.
