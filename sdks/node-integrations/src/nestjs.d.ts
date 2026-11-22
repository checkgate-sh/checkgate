import type { CanActivate, DynamicModule, ExecutionContext, InjectionToken, ModuleMetadata, OptionalFactoryDependency, PipeTransform } from '@nestjs/common'
import type { Reflector } from '@nestjs/core'
import type { CheckgateClientOptions } from '@checkgate/node'
import type {
  CheckgateClientLike, ContextExtractor, EvaluationContextInput, RequestFlags, RequestFlagsOptions, StartOptions,
  StartResult, TestFlags,
} from './index'

export type { RequestFlags, TestClient } from './index'
export { createTestClient, startCheckgate, stopCheckgate } from './index'

/** Injection token of the shared CheckgateClient. */
export declare const CHECKGATE_CLIENT: unique symbol
/** Injection token of the module options. */
export declare const CHECKGATE_OPTIONS: unique symbol
/** Metadata key holding RequireFlag() requirements. */
export declare const REQUIRE_FLAG_METADATA: 'checkgate:require-flag'

export interface CheckgateModuleOptions extends RequestFlagsOptions, StartOptions {
  /** A client you built; or */
  client?: CheckgateClientLike
  /** options to construct one. */
  clientOptions?: CheckgateClientOptions
  /**
   * Returns the request's `{ key, attributes }` from the platform request
   * (Express or Fastify). Called once per request, on first use.
   * Default: `{ key: req.user.id }`, else anonymous.
   */
  context?: ContextExtractor<any>
  /** Register the module globally. Default true. */
  isGlobal?: boolean
}

export interface CheckgateModuleAsyncOptions extends Pick<ModuleMetadata, 'imports'> {
  useFactory: (...args: any[]) => CheckgateModuleOptions | Promise<CheckgateModuleOptions>
  inject?: Array<InjectionToken | OptionalFactoryDependency>
  /** Default true. */
  isGlobal?: boolean
}

export declare class CheckgateModule {
  static forRoot(options: CheckgateModuleOptions): DynamicModule
  static forRootAsync(options: CheckgateModuleAsyncOptions): DynamicModule
  /** Backed by createTestClient(flags): no server, no native addon. */
  static forTesting(flags?: TestFlags, options?: Omit<CheckgateModuleOptions, 'client' | 'clientOptions'>): DynamicModule
}

export declare class CheckgateService {
  readonly client: CheckgateClientLike
  readonly options: CheckgateModuleOptions
  /** Result of startup, or null before bootstrap. */
  readonly startup: StartResult | null
  /** Flags for a platform request (one evaluator per request). */
  forRequest(req: unknown): RequestFlags
  /** Flags for an explicit context: jobs, consumers, scripts. */
  forContext(context: EvaluationContextInput | string | null): RequestFlags
  onApplicationBootstrap(): Promise<void>
  onApplicationShutdown(): void
}

export declare class CheckgateFlagGuard implements CanActivate {
  constructor(reflector: Reflector, checkgate: CheckgateService)
  canActivate(context: ExecutionContext): boolean
}

export declare class CheckgateRequestPipe implements PipeTransform {
  constructor(checkgate: CheckgateService)
  transform(req: unknown): RequestFlags
}

/**
 * The route answers only when `flagKey` is on for the request's user, else
 * 404 (or 403). On a class or a method; several must all pass.
 */
export declare function RequireFlag(flagKey: string, options?: { status?: 404 | 403 }): ClassDecorator & MethodDecorator

/** Parameter decorator: this request's {@link RequestFlags}. */
export declare function Checkgate(): ParameterDecorator
