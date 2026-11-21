// NestJS module against the real @checkgate/node client and the fake
// Checkgate server (see helpers.mjs). Plain JavaScript, so the decorators
// are applied by calling them, as TypeScript's output does.

import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { checkResponse, contextFromHeaders, frameworkSuite, get, makeClient, quietLogger, startFake } from './helpers.mjs'

const require = createRequire(import.meta.url)
require('reflect-metadata')
const { Controller, Get, Inject, Injectable, Module, UseGuards } = require('@nestjs/common')
const { Test } = require('@nestjs/testing')
const { FastifyAdapter } = require('@nestjs/platform-fastify')
const {
    CHECKGATE_CLIENT, Checkgate, CheckgateModule, CheckgateService, RequireFlag, createTestClient,
} = require('@checkgate/node-integrations/nestjs')

/** Applies method decorators like TypeScript's `__decorate` does. */
function decorate(Cls, method, decorators) {
    const proto = Cls.prototype
    const descriptor = Object.getOwnPropertyDescriptor(proto, method)
    Object.defineProperty(proto, method, Reflect.decorate(decorators, proto, method, descriptor) || descriptor)
}

class CheckController {
    async check(flags) {
        return checkResponse(flags)
    }
}
Controller()(CheckController)
decorate(CheckController, 'check', [Get('check')])
Checkgate()(CheckController.prototype, 'check', 0)

/** Stand-in for an auth guard: sets req.user from headers. */
class FakeAuthGuard {
    canActivate(ctx) {
        const req = ctx.switchToHttp().getRequest()
        if (req.headers['x-user']) {
            req.user = { id: req.headers['x-user'], plan: req.headers['x-plan'], beta: req.headers['x-beta'] === '1' }
        }
        return true
    }
}
Injectable()(FakeAuthGuard)

class GatedController {
    notFound() { return { ok: true } }
    forbidden() { return { ok: true } }
    both() { return { ok: true } }
    open(flags) { return { key: flags.context.key, pro: flags.isEnabled('pro-feature') } }
}
// Auth at controller level runs before the method-level RequireFlag guard.
Controller('gated')(GatedController)
UseGuards(FakeAuthGuard)(GatedController)
decorate(GatedController, 'notFound', [Get('not-found'), RequireFlag('pro-feature')])
decorate(GatedController, 'forbidden', [Get('forbidden'), RequireFlag('pro-feature', { status: 403 })])
decorate(GatedController, 'both', [Get('both'), RequireFlag('pro-feature'), RequireFlag('beta-only')])
decorate(GatedController, 'open', [Get('open'), RequireFlag('pro-feature')])
Checkgate()(GatedController.prototype, 'open', 0)

const userContext = (req) => (req.user ? { key: req.user.id, attributes: { plan: req.user.plan, beta: req.user.beta } } : null)

// Nest's default HTTP platform is Express; 'fastify' runs the same app on
// @nestjs/platform-fastify, where the request is a FastifyRequest.
async function listen(moduleRef, platform = 'express') {
    const app = platform === 'fastify'
        ? moduleRef.createNestApplication(new FastifyAdapter(), { logger: false })
        : moduleRef.createNestApplication({ logger: false })
    try {
        await app.listen(0, '127.0.0.1')
    } catch (err) {
        await app.close().catch(() => {})
        throw err
    }
    return { app, url: `http://127.0.0.1:${app.getHttpServer().address().port}` }
}

for (const platform of ['express', 'fastify']) {
    frameworkSuite(platform === 'express' ? 'nestjs' : 'nestjs/fastify-platform', {
        async start({ client, readyTimeoutMs, onTimeout }) {
            const moduleRef = await Test.createTestingModule({
                imports: [CheckgateModule.forRoot({
                    client, readyTimeoutMs, onTimeout, logger: quietLogger, context: (req) => contextFromHeaders(req.headers),
                })],
                controllers: [CheckController],
            }).compile()
            const { app, url } = await listen(moduleRef, platform)
            return { url, close: () => app.close() }
        },
    })
}

for (const platform of ['express', 'fastify']) test(`[nestjs/${platform}] RequireFlag: 404 / 403 when off, passes when on, all flags must pass`, async () => {
    const fake = await startFake()
    const moduleRef = await Test.createTestingModule({
        imports: [CheckgateModule.forRoot({ client: makeClient(fake.url), context: userContext, logger: quietLogger })],
        controllers: [GatedController],
    }).compile()
    const { app, url } = await listen(moduleRef, platform)
    try {
        const pro = { 'x-user': 'p', 'x-plan': 'pro' }
        const proBeta = { ...pro, 'x-beta': '1' }
        const free = { 'x-user': 'f', 'x-plan': 'free' }
        assert.equal((await get(url, '/gated/not-found', pro)).status, 200)
        assert.equal((await get(url, '/gated/not-found', free)).status, 404)
        assert.equal((await get(url, '/gated/not-found')).status, 404)
        assert.equal((await get(url, '/gated/forbidden', pro)).status, 200)
        assert.equal((await get(url, '/gated/forbidden', free)).status, 403)
        assert.equal((await get(url, '/gated/both', pro)).status, 404)
        assert.equal((await get(url, '/gated/both', proBeta)).status, 200)
        // Guard and parameter share one evaluator: the context the guard used.
        assert.deepEqual((await get(url, '/gated/open', pro)).body, { key: 'p', pro: true })
    } finally {
        await app.close()
        await fake.stop()
    }
})

test('[nestjs] forTesting(): no server, no addon; flags settable per test', async () => {
    const moduleRef = await Test.createTestingModule({
        imports: [CheckgateModule.forTesting({
            'pro-feature': ({ key }) => key === 'p',
            'beta-only': true,
            'banner-color': 'gold',
        }, { context: userContext })],
        controllers: [GatedController, CheckController],
    }).compile()
    const { app, url } = await listen(moduleRef)
    try {
        assert.equal((await get(url, '/gated/both', { 'x-user': 'p' })).status, 200)
        assert.equal((await get(url, '/gated/both', { 'x-user': 'q' })).status, 404)
        moduleRef.get(CHECKGATE_CLIENT).setFlag('beta-only', false)
        assert.equal((await get(url, '/gated/both', { 'x-user': 'p' })).status, 404)
        await app.close()
        assert.equal(moduleRef.get(CHECKGATE_CLIENT).disconnectCalls, 1)
    } finally {
        await app.close()
    }
})

test('[nestjs] overrideProvider(CHECKGATE_CLIENT) swaps in a test client', async () => {
    const fakeClient = createTestClient({ 'pro-feature': true })
    const moduleRef = await Test.createTestingModule({
        // clientOptions are never used: the provider is overridden.
        imports: [CheckgateModule.forRoot({ clientOptions: { serverUrl: 'http://127.0.0.1:9' }, context: userContext })],
        controllers: [GatedController],
    }).overrideProvider(CHECKGATE_CLIENT).useValue(fakeClient).compile()
    const { app, url } = await listen(moduleRef)
    try {
        assert.equal((await get(url, '/gated/not-found', { 'x-user': 'anyone' })).status, 200)
    } finally {
        await app.close()
    }
})

test('[nestjs] forRootAsync() with an injected config, and CheckgateService in another provider', async () => {
    const fake = await startFake()
    const CONFIG = 'APP_CONFIG'
    class ConfigModule {}
    Module({ providers: [{ provide: CONFIG, useValue: { url: fake.url } }], exports: [CONFIG] })(ConfigModule)

    class ReportJob {
        constructor(checkgate) { this.checkgate = checkgate }
        run(plan) { return this.checkgate.forContext({ key: 'nightly', attributes: { plan } }).isEnabled('pro-feature') }
    }
    Injectable()(ReportJob)
    Inject(CheckgateService)(ReportJob, undefined, 0)

    const moduleRef = await Test.createTestingModule({
        imports: [CheckgateModule.forRootAsync({
            imports: [ConfigModule],
            inject: [CONFIG],
            useFactory: (config) => ({ client: makeClient(config.url), readyTimeoutMs: 3000, onTimeout: 'fail', logger: quietLogger }),
        })],
        providers: [ReportJob],
    }).compile()
    const app = moduleRef.createNestApplication({ logger: false })
    await app.init()
    try {
        const service = app.get(CheckgateService)
        assert.deepEqual(service.startup, { ready: true, timedOut: false })
        assert.equal(app.get(ReportJob).run('pro'), true)
        assert.equal(app.get(ReportJob).run('free'), false)
    } finally {
        await app.close()
        await fake.stop()
    }
})

test('[nestjs] forRoot() without client or clientOptions fails at module creation', async () => {
    await assert.rejects(Test.createTestingModule({ imports: [CheckgateModule.forRoot({})] }).compile(),
        /needs client or clientOptions/)
})
