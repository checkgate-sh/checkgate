<p align="center">
  <img src="https://raw.githubusercontent.com/checkgate-sh/checkgate/main/assets/logo.svg" width="64" alt="Checkgate">
</p>

# @checkgate/node-integrations

Express, Fastify and NestJS integrations for the Checkgate Node.js SDK
([`@checkgate/node`](https://www.npmjs.com/package/@checkgate/node)): flags
bound to each request's user, and the client's startup and shutdown.

**Current release: 0.2.0**, the first release of this package (new in Checkgate
0.2.0). Changes are listed in the
[changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md).

## What it does

- **One shared client, one evaluator per request.** Every request gets a
  `RequestFlags` object bound to that request's user:
  `req.checkgate.isEnabled('new-checkout')`. Evaluations are synchronous calls
  into the client's in-memory flags: no network request per request or per
  evaluation.
- **Startup that does not hang.** The client's `connect()` keeps retrying while
  the server is unreachable; the integrations wait at most `readyTimeoutMs` for
  the first flag set, then continue (cached or default values, still syncing)
  or fail, as you choose. An SDK key rejected within that time fails startup.
- **Graceful shutdown.** The client is disconnected exactly once, however many
  times close runs.
- **Evaluations never throw.** A failure is reported to `onError` and the
  evaluation returns its default.

## Installation

```bash
npm install @checkgate/node @checkgate/node-integrations
```

Install the framework you use as well (`express`, `fastify`, or `@nestjs/common`
and `@nestjs/core`); each is an optional peer dependency.

| Import | For |
|---|---|
| `@checkgate/node-integrations/express` | Express (any `(req, res, next)` stack) |
| `@checkgate/node-integrations/fastify` | Fastify |
| `@checkgate/node-integrations/nestjs` | NestJS |
| `@checkgate/node-integrations` | the framework-independent core and `createTestClient()` |

## Express

```js
import express from 'express'
import { CheckgateClient } from '@checkgate/node'
import { checkgateMiddleware, listenWithCheckgate } from '@checkgate/node-integrations/express'

// One client for the whole process.
const client = new CheckgateClient({
  serverUrl: process.env.CHECKGATE_URL,
  sdkKey: process.env.CHECKGATE_SDK_KEY,
})

const app = express()
app.use(checkgateMiddleware({
  client,
  // req.user is set by your authentication (before or after this middleware).
  context: (req) => (req.user ? { key: req.user.id, attributes: { plan: req.user.plan } } : null),
}))

app.get('/', (req, res) => {
  res.json({
    newCheckout: req.checkgate.isEnabled('new-checkout'),
    theme: req.checkgate.getValue('checkout-theme', 'light'),
  })
})

// Waits up to 5 s for flags, then listens either way.
const { server, close } = await listenWithCheckgate(app, { client, port: 8080, readyTimeoutMs: 5000 })
console.log(`listening on port ${server.address().port}`)

// Stop accepting connections, finish requests in flight, then disconnect.
process.once('SIGTERM', () => close().then(() => process.exit(0)))
```

`listenWithCheckgate()` installs no signal handlers. If you create the HTTP
server yourself, call `startCheckgate(client, options)` before `listen()` and
`stopCheckgate(client)` after `server.close()` (same import). In TypeScript,
importing the module adds `checkgate: RequestFlags` to Express's `Request`.

## Fastify

```js
import Fastify from 'fastify'
import { checkgatePlugin } from '@checkgate/node-integrations/fastify'

const app = Fastify()

// Creates the client, connects while the plugin loads, disconnects on close.
await app.register(checkgatePlugin, {
  clientOptions: { serverUrl: process.env.CHECKGATE_URL, sdkKey: process.env.CHECKGATE_SDK_KEY },
  context: (request) => (request.user ? { key: request.user.id, attributes: { plan: request.user.plan } } : null),
  readyTimeoutMs: 5000,
})

app.get('/', async (request) => ({ newCheckout: request.checkgate.isEnabled('new-checkout') }))

const address = await app.listen({ port: 8080 })
console.log(`listening on ${address}`)
process.once('SIGTERM', () => app.close().then(() => process.exit(0)))
```

Pass `clientOptions` to let the plugin build the client, or `client` to use
one you built; either way it is disconnected in `onClose`. `fastify.checkgate`
is the shared client and `request.checkgate` the request's evaluator. The
plugin is not encapsulated, so routes in every child plugin see both.
`readyTimeoutMs` must be below Fastify's `pluginTimeout` (10 s by default),
otherwise registration fails.

## NestJS

```ts
import 'reflect-metadata'
import { Controller, Get, Module } from '@nestjs/common'
import { NestFactory } from '@nestjs/core'
import { Checkgate, CheckgateModule, RequireFlag, type RequestFlags } from '@checkgate/node-integrations/nestjs'

@Controller()
class AppController {
  @Get()
  home(@Checkgate() flags: RequestFlags) {
    return { newCheckout: flags.isEnabled('new-checkout') }
  }

  @Get('beta')
  @RequireFlag('beta') // 404 unless 'beta' is on for this request's user
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
    }),
  ],
  controllers: [AppController],
})
class AppModule {}

async function bootstrap() {
  const app = await NestFactory.create(AppModule)
  app.enableShutdownHooks() // SIGTERM/SIGINT run onApplicationShutdown: the client disconnects
  await app.listen(8080)
  console.log('listening on port 8080')
}
void bootstrap()
```

Compile with `"experimentalDecorators": true` (Nest's decorators). The module
is global by default (`isGlobal: true`): import it once, in the root module.
`CheckgateService` connects the client in `onApplicationBootstrap` (so
`app.listen()` waits, within `readyTimeoutMs`) and disconnects it in
`onApplicationShutdown`, which runs on `app.close()` and, with
`enableShutdownHooks()`, on SIGTERM/SIGINT.

`@RequireFlag()` evaluates in a guard, and Nest runs guards global, then
controller, then method. Put authentication at the global or controller level
(or list `@RequireFlag()` above `@UseGuards(AuthGuard)` on the method) so that
`req.user` is set when the flag is evaluated.

## Configuration

| Option | Where | Default | Description |
|---|---|---|---|
| `client` | all | none | The one `CheckgateClient` shared by the app (required for Express). |
| `clientOptions` | Fastify, NestJS | none | [`CheckgateClient` options](https://checkgate-sh.github.io/checkgate/sdks/nodejs#new-checkgateclient-options) to build the client; pass this or `client`, not both. |
| `context` | all | `{ key: req.user.id }` when there is a `req.user`, else anonymous | `(req) => ({ key, attributes })`, a bare key, or `null` for anonymous. |
| `defaultKey` | all | `'anonymous'` | User key when the context has none (missing or empty). |
| `onError` | all | logs to `console.error` | `(err, info)`, called when an evaluation, the `context` function or `track()` fails. |
| `readyTimeoutMs` | all | `5000` | How long startup waits for the first flag set (0 to 2^31 - 1 ms). |
| `onTimeout` | all | `'continue'` | `'continue'`: start with cached or default values and keep syncing. `'fail'`: disconnect and reject with `CheckgateStartupTimeoutError`. |
| `logger` | Express `listenWithCheckgate()`, NestJS | `console` | Receives the timeout warning (Fastify uses `fastify.log`). |
| `port`, `host` | Express `listenWithCheckgate()` | `0` (any free port), all interfaces | Where to listen. |
| `isGlobal` | NestJS | `true` | Register the module globally. |

## API overview

- **`RequestFlags`** (`req.checkgate`, `request.checkgate`, `@Checkgate()`):
  `isEnabled(flag)` (`false` if off, missing or on error), `getVariant(flag)`
  (`{ enabled, value }` or `null`), `getValue(flag, default)`,
  `track(eventKey, { value?, context? })`, and `context` (the normalized
  `{ key, attributes }`).
- **Lifecycle:** `startCheckgate(client, { readyTimeoutMs, onTimeout, logger })`
  resolves `{ ready, timedOut }`; `stopCheckgate(client)` disconnects once and
  returns whether this call did. Exported from every entry point.
- **Express:** `checkgateMiddleware(options)`,
  `listenWithCheckgate(app, options)` resolving `{ server, startup, close }`.
- **Fastify:** `checkgatePlugin`, also the default export. From TypeScript
  ES modules, use the named import: the default import does not type-check
  there.
- **NestJS:** `CheckgateModule.forRoot(options)`,
  `forRootAsync({ imports, inject, useFactory })`, `forTesting(flags)`;
  `CheckgateService` (`client`, `startup`, `forRequest(req)`,
  `forContext({ key, attributes })` for jobs and consumers); `@Checkgate()`;
  `@RequireFlag(flag, { status: 404 | 403 })`; the `CHECKGATE_CLIENT` and
  `CHECKGATE_OPTIONS` injection tokens.
- **Core:** `createRequestFlags()`, `normalizeAttributes()`,
  `normalizeContext()`, `defaultContext()`, `CheckgateStartupTimeoutError`.
- **Testing:** `createTestClient(flags)`, an in-memory client with no server
  and no native addon, usable as the `client` of any adapter:

  ```js
  import { createTestClient } from '@checkgate/node-integrations'

  const client = createTestClient({
    'new-checkout': true,                    // on
    'checkout-theme': 'dark',                // on, serving 'dark'
    beta: ({ key }) => key === 'alice',      // per user
  })
  client.setFlag('new-checkout', false)      // change a flag mid-test
  ```

  `null` means off and an unlisted key is a missing flag; `disconnectCalls`
  and `tracked` record what the app did.

TypeScript declarations are included for every entry point.

## Request context and isolation

- Nothing about a request is stored on the shared client. Each request gets
  its own `RequestFlags`; in NestJS it is cached on the request object, so a
  guard and a handler parameter share one context. No NestJS provider is
  REQUEST-scoped, so controllers and services are not rebuilt per request.
- `context(req)` is called once per request, on the first evaluation, so it
  sees `req.user` even when authentication runs after the Checkgate
  middleware. A `context` function that throws is reported to `onError` and the
  request is treated as anonymous.
- Attributes are converted before they reach the engine, which only accepts
  strings: strings are kept; finite numbers, bigints and booleans become
  strings (`30` becomes `"30"`, so numeric operators work); `null`,
  `undefined`, `NaN`, `Infinity`, objects, arrays, dates, symbols and functions
  are dropped (a rule on them behaves as if the attribute were missing).
  Convert dates yourself, for example with `toISOString()`.

## Errors and lifecycle

| | The server answers in time | It does not | It rejects the SDK key (401/403) |
|---|---|---|---|
| `onTimeout: 'continue'` (default) | starts with live flags | starts, logs a warning, serves the client's offline cache (if its `storage` has one) or defaults, and keeps syncing | startup fails with `CheckgateAuthError` if the rejection arrives within `readyTimeoutMs` |
| `onTimeout: 'fail'` | starts with live flags | startup fails with `CheckgateStartupTimeoutError` (`timeoutMs`, `state`); the client is disconnected | startup fails with `CheckgateAuthError` |

A key rejected after startup does not stop the app; watch
`client.getStatus()` or `client.getHealth()` to detect it. On shutdown each
adapter calls `client.disconnect()` exactly once.

## Compatibility

| | Supported |
|---|---|
| Node.js | 22.12 or later (`engines: ">=22.12.0"`); CI tests 22, 24 and 26 |
| `@checkgate/node` | `>=0.1.31 <0.3.0` |
| `express` | 5.x |
| `fastify` | 5.x (`fastify-plugin` is a dependency of this package) |
| `@nestjs/common`, `@nestjs/core` | 12.x, on the Express or Fastify platform |

This package is CommonJS and loads NestJS 12 (published as ES modules only)
with `require()`, which Node.js supports from 22.12. Your application can be
CommonJS or ES modules. Express 4, Fastify 4 and NestJS 11 are not tested and
not declared as supported peers. The adapters cover HTTP; for other Nest
transports use `CheckgateService.forContext()`.

## Security considerations

- Targeting attributes are evaluated in the process. They are sent to the
  Checkgate server only if the client is created with
  `sendEvaluationContext: true`; the user key is sent with impressions and
  goal events unless `reportImpressions` is off.
- Nothing a request supplies can make an evaluation throw: attribute values
  are coerced or dropped, `__proto__` keys are ignored, and errors go to
  `onError`.
- Keep the SDK key in configuration or a secret store, and use an `https://`
  `serverUrl` outside local development.
- `@RequireFlag()` answers 404 by default, so a disabled route looks like a
  missing one; use `{ status: 403 }` to say it exists.

## Documentation

- [Express, Fastify and NestJS guide](https://checkgate-sh.github.io/checkgate/sdks/node-frameworks)
- [Node.js SDK](https://checkgate-sh.github.io/checkgate/sdks/nodejs)
- [SDK diagnostics](https://checkgate-sh.github.io/checkgate/operations/sdk-diagnostics)
- [Changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md)
- Runnable example apps: [`examples/`](https://github.com/checkgate-sh/checkgate/tree/main/sdks/node-integrations/examples) in the repository

## Development

From `sdks/node-integrations` in a checkout of the repository. The tests run
the real `@checkgate/node` client and native addon against the fake Checkgate
server in `sdks/conformance/fault-server.mjs`:

```bash
(cd ../nodejs && npm ci && npm run build:debug)   # needs a Rust toolchain
npm ci
npm run typecheck
npm test
```

## License

Apache-2.0
