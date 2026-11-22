<p align="center">
  <img src="../../assets/logo.svg" width="64" alt="Checkgate">
</p>

# @checkgate/node-integrations

Express, Fastify and NestJS integrations for [`@checkgate/node`](../nodejs/README.md):

- **Flags bound to each request's user**: `req.checkgate.isEnabled('new-ui')`,
  from one shared client, with no network call per request or per evaluation.
- **Startup that does not hang**: waits at most `readyTimeoutMs` for flags, then
  continues (cached or default values, still syncing) or fails. An SDK key
  rejected within that time fails startup.
- **Graceful shutdown**: the client disconnects exactly once.

```bash
npm install @checkgate/node @checkgate/node-integrations
```

| Supported | |
|---|---|
| Node.js | 22.12+ (CI: 22, 24, 26) |
| `@checkgate/node` | `>=0.1.31 <0.3.0` |
| `express` | 5.x |
| `fastify` | 5.x |
| `@nestjs/common`, `@nestjs/core` | 12.x (Express or Fastify platform) |

## Express

```js
const { checkgateMiddleware, listenWithCheckgate } = require('@checkgate/node-integrations/express')

app.use(checkgateMiddleware({
  client, // one CheckgateClient for the process
  context: (req) => (req.user ? { key: req.user.id, attributes: { plan: req.user.plan } } : null),
}))
app.get('/', (req, res) => res.json({ v2: req.checkgate.isEnabled('new-ui') }))

const { close } = await listenWithCheckgate(app, { client, port: 8080, readyTimeoutMs: 5000, onTimeout: 'continue' })
process.once('SIGTERM', () => close().then(() => process.exit(0)))
```

## Fastify

```js
await fastify.register(require('@checkgate/node-integrations/fastify'), {
  clientOptions: { serverUrl: process.env.CHECKGATE_URL, sdkKey: process.env.CHECKGATE_SDK_KEY },
  context: (request) => (request.user ? { key: request.user.id } : null),
})
fastify.get('/', async (request) => ({ v2: request.checkgate.isEnabled('new-ui') }))
```

## NestJS

```ts
@Module({ imports: [CheckgateModule.forRoot({ clientOptions: { serverUrl, sdkKey }, context: (req) => req.user?.id })] })
class AppModule {}

@Get() page(@Checkgate() flags: RequestFlags) { return { v2: flags.isEnabled('new-ui') } }
@Get('beta') @RequireFlag('beta') beta() { /* 404 unless on for this user */ }
```

`forRootAsync()`, `CheckgateService`, `forTesting()` and the guard-ordering
rules are in the docs.

## Notes

- `context(req)` is called once per request, on the first evaluation, so it
  can read `req.user` set by auth middleware that runs later.
- Attributes are coerced for the evaluation core: finite numbers, bigints and
  booleans become strings; `null`, `undefined`, objects, arrays, dates and other
  values are dropped. Nothing a request supplies can make an evaluation throw.
- `createTestClient(flags)` (from the package root) is an in-memory client for
  tests: no server, no native addon.

Full guide: [Express, Fastify and NestJS](https://checkgate-sh.github.io/checkgate/sdks/node-frameworks).

## Examples

Runnable apps in [`examples/`](examples), reading `CHECKGATE_URL` and
`CHECKGATE_SDK_KEY` from the environment:

```bash
npm ci
CHECKGATE_URL=http://localhost:3000 CHECKGATE_SDK_KEY=<your SDK key> node examples/express/server.mjs
CHECKGATE_URL=http://localhost:3000 CHECKGATE_SDK_KEY=<your SDK key> node examples/fastify/server.mjs
npx tsc -p examples/nestjs && CHECKGATE_URL=http://localhost:3000 CHECKGATE_SDK_KEY=<your SDK key> node examples/nestjs/dist/main.js
```

## Development

The tests run the real `@checkgate/node` client and native addon against the
fake Checkgate server in `sdks/conformance/fault-server.mjs`:

```bash
(cd ../nodejs && npm ci && npm run build:debug)   # needs a Rust toolchain
npm ci
npm run typecheck
npm test
```
