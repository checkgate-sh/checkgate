'use strict'

// NestJS module. Plain JavaScript: the decorators are applied by calling them,
// which is what TypeScript compiles `@Injectable()` / `@Inject(token)` into.
// Every constructor parameter names its token with Inject(), so nothing
// depends on `emitDecoratorMetadata`.
//
// No provider here is REQUEST-scoped. A request-scoped provider makes every
// provider that injects it request-scoped too, so Nest would rebuild those
// controllers and services on every request. Instead the singleton
// CheckgateService hands out a small per-request evaluator, cached on the
// request object itself (never on the shared client).

const {
    ForbiddenException,
    Inject,
    Injectable,
    Module,
    NotFoundException,
    UseGuards,
    createParamDecorator,
} = require('@nestjs/common')
const { GUARDS_METADATA } = require('@nestjs/common/constants')
const { Reflector } = require('@nestjs/core')
const { createRequestFlags, defaultContext, startCheckgate, stopCheckgate } = require('./core.js')
const { createTestClient } = require('./testing.js')

/** Injection token of the shared CheckgateClient. */
const CHECKGATE_CLIENT = Symbol('CHECKGATE_CLIENT')
/** Injection token of the options passed to forRoot() / returned by forRootAsync()'s factory. */
const CHECKGATE_OPTIONS = Symbol('CHECKGATE_OPTIONS')
/** Metadata key holding RequireFlag() requirements. */
const REQUIRE_FLAG_METADATA = 'checkgate:require-flag'

// Where the request's evaluator is cached, so a guard and a handler parameter
// in the same request share one context (resolved once).
const REQUEST_FLAGS = Symbol('checkgate.requestFlags')

function injectParams(target, tokens) {
    tokens.forEach((token, index) => Inject(token)(target, undefined, index))
}

/** The request object of an execution context (the first argument outside HTTP). */
function requestOf(ctx) {
    return ctx.getType() === 'http' ? ctx.switchToHttp().getRequest() : ctx.getArgByIndex(0)
}

/**
 * The application's entry point to Checkgate: owns the client's lifecycle and
 * hands out request-bound evaluators.
 */
class CheckgateService {
    constructor(client, options) {
        this.client = client
        this.options = options || {}
        /** The result of startup: `{ ready, timedOut }`, or null before bootstrap. */
        this.startup = null
        this._context = this.options.context || defaultContext
        this._evalOptions = { defaultKey: this.options.defaultKey, onError: this.options.onError }
    }

    /** Flags for the request `req` (cached on it: one context per request). */
    forRequest(req) {
        if (req === null || typeof req !== 'object') return this.forContext(null)
        let flags = req[REQUEST_FLAGS]
        if (!flags) {
            flags = createRequestFlags(this.client, () => this._context(req), this._evalOptions)
            Object.defineProperty(req, REQUEST_FLAGS, { value: flags, configurable: true })
        }
        return flags
    }

    /** Flags for an explicit context (`{ key, attributes }` or a key): jobs, consumers, scripts. */
    forContext(context) {
        return createRequestFlags(this.client, () => context, this._evalOptions)
    }

    async onApplicationBootstrap() {
        const { readyTimeoutMs, onTimeout, logger } = this.options
        this.startup = await startCheckgate(this.client, { readyTimeoutMs, onTimeout, logger })
    }

    // Runs on app.close(), and on SIGTERM/SIGINT only with app.enableShutdownHooks().
    onApplicationShutdown() {
        stopCheckgate(this.client)
    }
}
Injectable()(CheckgateService)
injectParams(CheckgateService, [CHECKGATE_CLIENT, CHECKGATE_OPTIONS])

/**
 * Guard behind RequireFlag(): lets the request through only if every
 * required flag is on for the request's user; otherwise 404 (default) or 403.
 */
class CheckgateFlagGuard {
    constructor(reflector, checkgate) {
        this.reflector = reflector
        this.checkgate = checkgate
    }

    canActivate(ctx) {
        const required = this.reflector.getAllAndMerge(REQUIRE_FLAG_METADATA, [ctx.getClass(), ctx.getHandler()])
        if (!required || required.length === 0) return true
        const flags = this.checkgate.forRequest(requestOf(ctx))
        for (const { flag, status } of required) {
            if (flags.isEnabled(flag)) continue
            throw status === 403 ? new ForbiddenException() : new NotFoundException()
        }
        return true
    }
}
Injectable()(CheckgateFlagGuard)
injectParams(CheckgateFlagGuard, [Reflector, CheckgateService])

/**
 * Method or controller decorator: the route answers only when `flag` is on
 * for the request's user, else 404 (or 403 with `{ status: 403 }`). Several
 * RequireFlag()s (on the class and the method) must all pass.
 *
 * Guards run in order: global, controller, then method guards, and method
 * decorators apply bottom-up. Put authentication at the controller or global
 * level, or list RequireFlag() above UseGuards(AuthGuard) on the method, so
 * `req.user` is set when the flag is evaluated.
 */
function RequireFlag(flag, { status = 404 } = {}) {
    if (typeof flag !== 'string' || flag.length === 0) throw new TypeError('[Checkgate] RequireFlag() needs a flag key.')
    if (status !== 404 && status !== 403) throw new TypeError('[Checkgate] RequireFlag() status must be 404 or 403.')
    return (target, key, descriptor) => {
        const where = descriptor ? descriptor.value : target
        const existing = Reflect.getMetadata(REQUIRE_FLAG_METADATA, where) || []
        Reflect.defineMetadata(REQUIRE_FLAG_METADATA, [...existing, { flag, status }], where)
        // One guard per target however many flags it requires.
        const guards = Reflect.getMetadata(GUARDS_METADATA, where) || []
        if (!guards.includes(CheckgateFlagGuard)) UseGuards(CheckgateFlagGuard)(target, key, descriptor)
        return descriptor
    }
}

/** Turns the request into its evaluator; a pipe so it is built by Nest's injector. */
class CheckgateRequestPipe {
    constructor(checkgate) {
        this.checkgate = checkgate
    }

    transform(req) {
        return this.checkgate.forRequest(req)
    }
}
Injectable()(CheckgateRequestPipe)
injectParams(CheckgateRequestPipe, [CheckgateService])

const RequestParam = createParamDecorator((_data, ctx) => requestOf(ctx))

/**
 * Parameter decorator: the flags for this request's user, e.g.
 * `handler(@Checkgate() flags: RequestFlags)`.
 */
function Checkgate() {
    return RequestParam(CheckgateRequestPipe)
}

function clientProvider() {
    return {
        provide: CHECKGATE_CLIENT,
        useFactory: (options) => {
            if (options.client && options.clientOptions) {
                throw new TypeError('[Checkgate] Pass either client or clientOptions, not both.')
            }
            if (options.client) return options.client
            if (!options.clientOptions) throw new TypeError('[Checkgate] CheckgateModule needs client or clientOptions.')
            // Loaded only here, so forTesting() and a supplied client need no native addon.
            const { CheckgateClient } = require('@checkgate/node')
            return new CheckgateClient(options.clientOptions)
        },
        inject: [CHECKGATE_OPTIONS],
    }
}

function dynamicModule(optionsProvider, { isGlobal = true, imports = [], extraProviders = [] } = {}) {
    return {
        module: CheckgateModule,
        global: isGlobal,
        imports,
        providers: [optionsProvider, clientProvider(), CheckgateService, CheckgateFlagGuard, CheckgateRequestPipe, ...extraProviders],
        exports: [CHECKGATE_CLIENT, CHECKGATE_OPTIONS, CheckgateService, CheckgateFlagGuard, CheckgateRequestPipe],
    }
}

class CheckgateModule {
    /**
     * @param {object} options - `{ client | clientOptions, context, defaultKey, onError,
     *        readyTimeoutMs, onTimeout, logger, isGlobal = true }`
     */
    static forRoot(options = {}) {
        return dynamicModule({ provide: CHECKGATE_OPTIONS, useValue: options }, { isGlobal: options.isGlobal !== false })
    }

    /**
     * @param {object} options - `{ useFactory(...deps) => options, inject?, imports?, isGlobal = true }`
     */
    static forRootAsync({ useFactory, inject = [], imports = [], isGlobal = true } = {}) {
        if (typeof useFactory !== 'function') throw new TypeError('[Checkgate] forRootAsync() needs useFactory.')
        return dynamicModule({ provide: CHECKGATE_OPTIONS, useFactory, inject }, { isGlobal: isGlobal !== false, imports })
    }

    /**
     * A module backed by createTestClient(flags): no server, no native addon.
     * The client is exported under CHECKGATE_CLIENT, so a test can change
     * flags with `moduleRef.get(CHECKGATE_CLIENT).setFlag(key, value)`.
     */
    static forTesting(flags = {}, options = {}) {
        return CheckgateModule.forRoot({ ...options, client: createTestClient(flags), clientOptions: undefined })
    }
}
Module({})(CheckgateModule)

module.exports = {
    CHECKGATE_CLIENT,
    CHECKGATE_OPTIONS,
    REQUIRE_FLAG_METADATA,
    Checkgate,
    CheckgateFlagGuard,
    CheckgateModule,
    CheckgateRequestPipe,
    CheckgateService,
    RequireFlag,
    createTestClient,
    startCheckgate,
    stopCheckgate,
}
