// Compile-only check of the hand-written .d.ts files: `npm run typecheck`.
// Mirrors how an application uses each adapter, so a declaration that does
// not match real usage fails here.

import express from 'express'
import Fastify from 'fastify'
import { Controller, Get, Injectable, Module, UseGuards } from '@nestjs/common'
import { Test } from '@nestjs/testing'
import { CheckgateClient } from '@checkgate/node'
import {
  createRequestFlags, createTestClient, normalizeAttributes, startCheckgate, stopCheckgate,
  type RequestFlags, type StartResult,
} from '@checkgate/node-integrations'
import { checkgateMiddleware, listenWithCheckgate } from '@checkgate/node-integrations/express'
import checkgatePlugin from '@checkgate/node-integrations/fastify'
import {
  CHECKGATE_CLIENT, Checkgate, CheckgateModule, CheckgateService, RequireFlag, type TestClient,
} from '@checkgate/node-integrations/nestjs'

declare global {
  namespace Express {
    // What auth middleware such as Passport declares.
    interface Request { user?: { id: string; plan: string } }
  }
}

const client = new CheckgateClient({ serverUrl: 'http://localhost:3000', sdkKey: 'sk_test' })

// -- core -------------------------------------------------------------------
async function core(): Promise<void> {
  const result: StartResult = await startCheckgate(client, { readyTimeoutMs: 1000, onTimeout: 'fail' })
  const stopped: boolean = stopCheckgate(client)
  const attrs: Record<string, string> = normalizeAttributes({ seats: 3 })
  const flags: RequestFlags = createRequestFlags(client, () => ({ key: 'u', attributes: { seats: 3, beta: true } }))
  const on: boolean = flags.isEnabled('x')
  const color = flags.getValue('color', 'blue')
  const variant = flags.getVariant('color')
  flags.track('goal', { value: 1 })
  const key: string = flags.context.key
  const fake: TestClient = createTestClient({ a: true, b: ({ key }) => key === 'u' })
  fake.setFlag('a', false)
  void [result, stopped, attrs, on, color, variant, key]
}

// -- express ----------------------------------------------------------------
async function expressApp(): Promise<void> {
  const app = express()
  app.use(checkgateMiddleware({
    client,
    context: (req: express.Request) => ({ key: req.user?.id, attributes: { plan: req.user?.plan } }),
  }))
  app.get('/', (req, res) => {
    res.json({ on: req.checkgate.isEnabled('new-ui') })
  })
  const { server, startup, close } = await listenWithCheckgate(app, { client, port: 3000, onTimeout: 'continue' })
  process.once('SIGTERM', () => void close())
  void [server.address(), startup.ready]
}

// -- fastify ----------------------------------------------------------------
async function fastifyApp(): Promise<void> {
  const app = Fastify()
  await app.register(checkgatePlugin, {
    client,
    context: (request) => ({ key: String(request.headers['x-user'] ?? '') }),
    readyTimeoutMs: 3000,
  })
  await app.register(checkgatePlugin, { clientOptions: { serverUrl: 'http://localhost:3000' } })
  app.get('/', async (request) => ({ on: request.checkgate.isEnabled('new-ui'), shared: app.checkgate.isReady() }))
  await app.listen({ port: 3000 })
}

// -- nestjs -----------------------------------------------------------------
@Injectable()
class AuthGuard {
  canActivate(): boolean { return true }
}

@Controller('beta')
@UseGuards(AuthGuard)
class BetaController {
  constructor(private readonly checkgate: CheckgateService) {}

  @Get()
  @RequireFlag('beta-page')
  page(@Checkgate() flags: RequestFlags) {
    return { color: flags.getValue('color', 'blue'), job: this.checkgate.forContext('job').isEnabled('x') }
  }
}

@Module({
  imports: [
    CheckgateModule.forRoot({ clientOptions: { serverUrl: 'http://localhost:3000' }, readyTimeoutMs: 3000 }),
    CheckgateModule.forRootAsync({ useFactory: () => ({ client }), inject: [] }),
  ],
  controllers: [BetaController],
})
class AppModule {}

async function nestTest(): Promise<void> {
  const moduleRef = await Test.createTestingModule({
    imports: [CheckgateModule.forTesting({ 'beta-page': true })],
    controllers: [BetaController],
  }).compile()
  moduleRef.get<TestClient>(CHECKGATE_CLIENT).setFlag('beta-page', false)
}

void [core, expressApp, fastifyApp, nestTest, AppModule]
