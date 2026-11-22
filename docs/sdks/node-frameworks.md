---
title: "Express, Fastify and NestJS — Checkgate Feature Flags in Node.js Frameworks"
description: "Use Checkgate feature flags in Express, Fastify and NestJS with @checkgate/node-integrations: per-request flag evaluation bound to the current user, startup with a timeout policy, graceful shutdown, and test doubles."
---

# Express, Fastify and NestJS <Since v="0.2.0" />

<ReleaseNotice pkg="node-integrations" />

`@checkgate/node-integrations` connects the [Node.js SDK](/sdks/nodejs) to a web
framework. It gives every request a small evaluator bound to that request's
user, starts the client with a timeout so your server never hangs waiting for
Checkgate, and disconnects it once when the app shuts down.

```bash
npm install @checkgate/node @checkgate/node-integrations
```

| Import | For |
|---|---|
| `@checkgate/node-integrations/express` | Express (any `(req, res, next)` stack) |
| `@checkgate/node-integrations/fastify` | Fastify |
| `@checkgate/node-integrations/nestjs` | NestJS |
| `@checkgate/node-integrations` | the framework-independent core and `createTestClient()` |

Each framework is an optional peer dependency: install only the one you use.

| Package | Supported |
|---|---|
| Node.js | 22.12 or later (tested on 22, 24 and 26) |
| `@checkgate/node` | 0.1.31 or later, below 0.3 |
| `express` | 5.x |
| `fastify` | 5.x (`fastify-plugin` is a dependency of this package) |
| `@nestjs/common`, `@nestjs/core` | 12.x, on the Express or Fastify platform |

NestJS 12 is published as ES modules only; this package is CommonJS and loads
it with `require()`, which Node.js supports from 22.12. Your app can be either
CommonJS or ES modules.

## How it works

- **One client per process.** You create one `CheckgateClient` (or let the
  Fastify plugin / Nest module create it). It holds one stream to the server
  and the flags in memory.
- **One evaluator per request.** `req.checkgate` (Express),
  `request.checkgate` (Fastify) or `@Checkgate()` (NestJS) is a
  `RequestFlags` object: `isEnabled(flag)`, `getValue(flag, default)`,
  `getVariant(flag)`, `track(event, opts)` and `context`. Nothing about the
  request is stored on the shared client.
- **No network call per request.** Every evaluation is a synchronous lookup
  in the client's memory. Flag changes arrive over the client's stream.
  (Impressions, if you leave `reportImpressions` on, are batched and sent
  by the SDK in the background, not per request.)
- **Evaluations never throw.** A failure is passed to `onError` (default:
  `console.error`) and the evaluation returns its default: `false`, `null`,
  or the default you passed to `getValue()`.

### The request context

You supply a `context(req)` function that returns the user key (used for
rollout hashing) and the attributes your targeting rules read:

```js
context: (req) => req.user
  ? { key: req.user.id, attributes: { plan: req.user.plan, seats: req.user.seats } }
  : null // anonymous
```

It is called **once per request, on the first evaluation**, so it sees
`req.user` even when your authentication middleware runs after the Checkgate
one. Without a `context` option the default is `{ key: req.user.id }` when
there is a `req.user`. A missing or empty key becomes `defaultKey`
(`'anonymous'` unless you set it), and a `context` that throws is reported to
`onError` and treated as anonymous.

The evaluation core only accepts string attributes. Values are converted
before they reach it:

| Attribute value | Sent as |
|---|---|
| string | unchanged |
| finite number, bigint | decimal string (`30` → `"30"`), so numeric operators like `greater_than` work |
| boolean | `"true"` / `"false"` |
| `null`, `undefined`, `NaN`, `Infinity`, objects, arrays, dates, symbols, functions | dropped: a rule on it behaves as if the attribute were missing |

Convert dates and nested values yourself (`createdAt: user.createdAt.toISOString()`).

### Startup and shutdown

`client.connect()` keeps retrying while the Checkgate server is unreachable, so
every adapter waits at most `readyTimeoutMs` (default 5000) for the first flag
set and then follows `onTimeout`:

| | The server answers in time | It does not | It rejects the SDK key (401/403) |
|---|---|---|---|
| `onTimeout: 'continue'` (default) | starts with live flags | starts anyway, logs a warning, serves the [offline cache](/sdks/nodejs#resilience-offline) if the client has a `storage` adapter with flags, else defaults, and keeps syncing in the background | startup fails with `CheckgateAuthError` if the rejection arrives within `readyTimeoutMs`; a later rejection is logged and the app keeps serving cached flags or defaults |
| `onTimeout: 'fail'` | starts with live flags | startup fails with `CheckgateStartupTimeoutError`; the client is disconnected | startup fails with `CheckgateAuthError` |

A key rejected before the timeout fails startup: it does not fix itself by waiting. Watch
`client.getStatus()` (or `getHealth()`) to catch a rejection that arrives later. On
shutdown each adapter calls `client.disconnect()` exactly once, however many
times close runs.

## Express

```js
import express from 'express'
import { CheckgateClient } from '@checkgate/node'
import { checkgateMiddleware, listenWithCheckgate } from '@checkgate/node-integrations/express'

const client = new CheckgateClient({
  serverUrl: process.env.CHECKGATE_URL,
  sdkKey: process.env.CHECKGATE_SDK_KEY,
})

const app = express()
app.use(checkgateMiddleware({
  client,
  context: (req) => (req.user ? { key: req.user.id, attributes: { plan: req.user.plan } } : null),
}))

app.get('/checkout', (req, res) => {
  res.json({ ui: req.checkgate.isEnabled('new-checkout-flow') ? 'v2' : 'v1' })
})

const { server, close } = await listenWithCheckgate(app, {
  client,
  port: 8080,
  readyTimeoutMs: 5000,
  onTimeout: 'continue',
})

process.once('SIGTERM', () => close().then(() => process.exit(0)))
```

`listenWithCheckgate()` starts Checkgate first (with the policy above), then
listens. Its `close()` stops accepting connections, lets requests in flight
finish (with flags still served), then disconnects the client. It installs no
signal handlers; wire them as shown. If you create the server yourself, use
`startCheckgate(client, options)` before `listen()` and
`stopCheckgate(client)` after `server.close()`, from the same import.

TypeScript: importing the module adds `checkgate: RequestFlags` to Express's
`Request` type.

## Fastify

```js
import Fastify from 'fastify'
import checkgate from '@checkgate/node-integrations/fastify'

const app = Fastify()

await app.register(checkgate, {
  clientOptions: { serverUrl: process.env.CHECKGATE_URL, sdkKey: process.env.CHECKGATE_SDK_KEY },
  context: (request) => (request.user ? { key: request.user.id, attributes: { plan: request.user.plan } } : null),
  readyTimeoutMs: 5000,
  onTimeout: 'continue',
})

app.get('/checkout', async (request) => ({
  ui: request.checkgate.isEnabled('new-checkout-flow') ? 'v2' : 'v1',
}))

await app.listen({ port: 8080 })
process.once('SIGTERM', () => app.close().then(() => process.exit(0)))
```

- Pass `clientOptions` and the plugin builds the client, or `client` to use
  one you built. Either way it connects while the plugin loads, so
  `ready()` / `listen()` wait for it (up to `readyTimeoutMs`), and it
  disconnects in `onClose`.
- `fastify.checkgate` is the shared client; `request.checkgate` is the
  request's evaluator (assigned in an `onRequest` hook, so every request gets
  its own).
- The plugin is not encapsulated: routes in every child plugin see the
  decorators.
- `readyTimeoutMs` must be below Fastify's `pluginTimeout` (10 s by default),
  otherwise registration fails: Fastify would abort the plugin with its own
  timeout first.
- TypeScript: importing the module augments `FastifyInstance` and
  `FastifyRequest`.

## NestJS

```ts
import { Controller, Get, Module, UseGuards } from '@nestjs/common'
import { NestFactory } from '@nestjs/core'
import { Checkgate, CheckgateModule, RequireFlag, type RequestFlags } from '@checkgate/node-integrations/nestjs'

@Controller('checkout')
@UseGuards(AuthGuard) // sets req.user
class CheckoutController {
  @Get()
  page(@Checkgate() flags: RequestFlags) {
    return { ui: flags.isEnabled('new-checkout-flow') ? 'v2' : 'v1' }
  }

  @Get('beta')
  @RequireFlag('checkout-beta') // 404 unless on for this user
  beta() {
    return { beta: true }
  }
}

@Module({
  imports: [
    CheckgateModule.forRoot({
      clientOptions: { serverUrl: process.env.CHECKGATE_URL!, sdkKey: process.env.CHECKGATE_SDK_KEY },
      context: (req) => (req.user ? { key: req.user.id, attributes: { plan: req.user.plan } } : null),
      readyTimeoutMs: 5000,
      onTimeout: 'continue',
    }),
  ],
  controllers: [CheckoutController],
})
class AppModule {}

const app = await NestFactory.create(AppModule)
app.enableShutdownHooks() // SIGTERM/SIGINT then run onApplicationShutdown
await app.listen(8080)
```

- **`CheckgateModule.forRoot(options)`** takes `client` or `clientOptions`,
  `context`, `defaultKey`, `onError`, `readyTimeoutMs`, `onTimeout`,
  `logger`, and `isGlobal` (default `true`: import it once, in the root
  module). **`forRootAsync({ imports, inject, useFactory })`** builds the same
  options from other providers, e.g. a config service.
- The client is a singleton under the exported `CHECKGATE_CLIENT` token.
  **`CheckgateService`** connects it in `onApplicationBootstrap` (so
  `app.listen()` / `app.init()` waits, with the timeout policy) and
  disconnects it in `onApplicationShutdown`. That hook runs on `app.close()`;
  for SIGTERM/SIGINT call `app.enableShutdownHooks()`.
- **`@Checkgate()`** is a parameter decorator that injects the request's
  `RequestFlags`. Outside HTTP handlers (jobs, queue consumers) inject
  `CheckgateService` and use `forContext({ key, attributes })`.
- **`@RequireFlag(flag, { status })`** on a method or controller answers 404
  (or 403 with `{ status: 403 }`) unless the flag is on for the request's
  user. Several must all pass. It evaluates in a guard, and guards run
  global → controller → method, with method decorators applied bottom-up: put
  authentication at the controller or global level, or list `@RequireFlag()`
  above `@UseGuards(AuthGuard)` on the same method, so `req.user` is set
  first.
- **No REQUEST-scoped providers.** A request-scoped provider makes every
  provider that injects it request-scoped too, so Nest would re-create those
  controllers and services on every request. The module's providers are all
  singletons; the per-request evaluator is created by a guard or pipe and
  cached on the request object, so a guard and a handler parameter in the
  same request share one context.

### Testing a Nest app

`CheckgateModule.forTesting(flags)` replaces the client with an in-memory one:
no Checkgate server, no native addon.

```ts
const moduleRef = await Test.createTestingModule({
  imports: [CheckgateModule.forTesting({
    'new-checkout-flow': true,
    'banner-color': 'gold',
    'checkout-beta': ({ key }) => key === 'beta-tester',
  }, { context: (req) => req.user?.id })],
  controllers: [CheckoutController],
}).compile()

moduleRef.get<TestClient>(CHECKGATE_CLIENT).setFlag('new-checkout-flow', false)
```

Or keep your real module and override the token:
`.overrideProvider(CHECKGATE_CLIENT).useValue(createTestClient({ ... }))`.

## Testing with other frameworks

`createTestClient(flags)` from `@checkgate/node-integrations` works as the
`client` of any adapter. A value of `true`/`false` is a boolean flag on/off,
any other value means on and serving that value, `null` means off, and a key
that is not listed is a missing flag. A function receives `{ key, attributes }`
for per-user answers. `setFlag(key, value)` changes a flag mid-test;
`disconnectCalls` and `tracked` record what the app did.

## Limitations

- Express 4, Fastify 4 and NestJS 11 are not tested and not declared as
  supported peers.
- The adapters cover HTTP. For other Nest transports, use
  `CheckgateService.forContext()`.
