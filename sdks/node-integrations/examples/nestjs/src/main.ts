// Minimal NestJS app with Checkgate.
//
//   npx tsc -p examples/nestjs
//   CHECKGATE_URL=http://localhost:3000 CHECKGATE_SDK_KEY=<your SDK key> node examples/nestjs/dist/main.js
//   curl -H 'x-user-id: alice' -H 'x-plan: pro' http://localhost:8080/
//   curl -H 'x-user-id: alice' http://localhost:8080/beta      # 404 unless beta-page is on for alice

import 'reflect-metadata'
import { Controller, Get, Injectable, Module, UseGuards, type CanActivate, type ExecutionContext } from '@nestjs/common'
import { NestFactory } from '@nestjs/core'
import { Checkgate, CheckgateModule, RequireFlag, type RequestFlags } from '@checkgate/node-integrations/nestjs'

interface DemoRequest {
  headers: Record<string, string | string[] | undefined>
  user?: { id: string; plan: string }
}

/** Stand-in for real authentication, which would set req.user. */
@Injectable()
class DemoAuthGuard implements CanActivate {
  canActivate(ctx: ExecutionContext): boolean {
    const req = ctx.switchToHttp().getRequest<DemoRequest>()
    const id = req.headers['x-user-id']
    if (typeof id === 'string') {
      const plan = req.headers['x-plan']
      req.user = { id, plan: typeof plan === 'string' ? plan : 'free' }
    }
    return true
  }
}

// Authentication at controller level runs before the method's RequireFlag guard.
@Controller()
@UseGuards(DemoAuthGuard)
class AppController {
  @Get()
  home(@Checkgate() flags: RequestFlags) {
    return {
      user: flags.context.key,
      newCheckout: flags.isEnabled('new-checkout'),
      bannerColor: flags.getValue('banner-color', 'blue'),
    }
  }

  @Get('beta')
  @RequireFlag('beta-page')
  beta() {
    return { welcome: 'to the beta' }
  }
}

@Module({
  imports: [
    CheckgateModule.forRoot({
      clientOptions: {
        serverUrl: process.env.CHECKGATE_URL ?? 'http://localhost:3000',
        sdkKey: process.env.CHECKGATE_SDK_KEY,
      },
      context: (req: DemoRequest) => (req.user ? { key: req.user.id, attributes: { plan: req.user.plan } } : null),
      // Waits up to 5 s during bootstrap, then starts with cached or default values.
      readyTimeoutMs: 5000,
      onTimeout: 'continue',
    }),
  ],
  controllers: [AppController],
})
class AppModule {}

async function bootstrap(): Promise<void> {
  const app = await NestFactory.create(AppModule, { logger: ['warn', 'error'] })
  // SIGTERM / SIGINT run onApplicationShutdown, which disconnects the client.
  app.enableShutdownHooks()
  await app.listen(Number(process.env.PORT ?? 8080))
  console.log(`listening on ${await app.getUrl()}`)
}

void bootstrap()
