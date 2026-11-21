import type { CheckgateClient, FlagEvaluation, FlagValue } from '@checkgate/node'

export type { FlagEvaluation, FlagValue }

/**
 * The part of a CheckgateClient the integrations use. A real
 * `CheckgateClient` satisfies it, and so does {@link createTestClient}.
 */
export interface CheckgateClientLike {
  connect(): Promise<void>
  disconnect(): void
  isReady(): boolean
  isEnabled(flagKey: string, userKey: string, userAttributes?: Record<string, string>): boolean
  getVariant(flagKey: string, userKey: string, userAttributes?: Record<string, string>): FlagEvaluation | null
  getStatus?(): { state: string }
  track?(eventKey: string, userKey: string, opts?: { value?: number; context?: Record<string, unknown> }): void
}

/** An attribute value as the extractor may return it; see {@link normalizeAttributes}. */
export type AttributeInput = string | number | bigint | boolean | null | undefined | object

/** What a `context(req)` extractor returns. */
export interface EvaluationContextInput {
  /** Stable user key used for rollout hashing. Missing or empty: `defaultKey`. */
  key?: string | number | bigint | null
  /** Targeting attributes; coerced by {@link normalizeAttributes}. */
  attributes?: Record<string, AttributeInput> | null
}

/** A context extractor: `{ key, attributes }`, a bare key, or nothing (anonymous). */
export type ContextExtractor<Req> = (req: Req) => EvaluationContextInput | string | number | null | undefined

/** The normalized context an evaluator uses. */
export interface EvaluationContext {
  readonly key: string
  readonly attributes: Readonly<Record<string, string>>
}

export interface ErrorInfo {
  /** The flag being evaluated, when the error came from an evaluation. */
  flag?: unknown
  /** `'context'` (the extractor threw) or `'track'`. */
  phase?: 'context' | 'track'
}

export interface RequestFlagsOptions {
  /** User key when the context has none. Default `'anonymous'`. */
  defaultKey?: string
  /** Called when an evaluation fails; the evaluation returns its default. Default: console.error. */
  onError?: (err: unknown, info: ErrorInfo) => void
}

/**
 * The flags of one request, bound to that request's user. Every method is
 * synchronous, makes no network call, and never throws.
 */
export declare class RequestFlags {
  constructor(client: CheckgateClientLike, resolveContext: () => unknown, options?: RequestFlagsOptions)
  /** Resolved from the extractor on first use, then fixed for the request. */
  readonly context: EvaluationContext
  /** `true` if the flag is on for this user; `false` if off, missing, or on error. */
  isEnabled(flagKey: string): boolean
  /** `{ enabled, value }`, or `null` if the flag is missing (or on error). */
  getVariant(flagKey: string): FlagEvaluation | null
  /** The resolved value, or `defaultValue` if the flag is missing (or on error). */
  getValue<T extends FlagValue = FlagValue>(flagKey: string, defaultValue?: T): FlagValue | T
  /** Records a goal event for this user. */
  track(eventKey: string, opts?: { value?: number; context?: Record<string, unknown> }): void
}

export declare function createRequestFlags(
  client: CheckgateClientLike,
  resolveContext: () => unknown,
  options?: RequestFlagsOptions
): RequestFlags

/** The default extractor: `{ key: req.user.id }` when there is a `req.user`, else anonymous. */
export declare function defaultContext(req: unknown): EvaluationContextInput | null

/**
 * Strings stay; finite numbers, bigints and booleans become strings; every
 * other value (null, undefined, NaN, Infinity, objects, arrays, dates,
 * symbols, functions) is dropped. Never throws.
 */
export declare function normalizeAttributes(input: unknown): Record<string, string>

export declare function normalizeContext(raw: unknown, defaultKey?: string): EvaluationContext

export interface StartOptions {
  /** How long startup waits for the first flag set. Default 5000. */
  readyTimeoutMs?: number
  /**
   * After the timeout: `'continue'` (default) starts with cached or default
   * values and keeps syncing in the background; `'fail'` disconnects and
   * rejects with {@link CheckgateStartupTimeoutError}. A rejected SDK key
   * always rejects.
   */
  onTimeout?: 'continue' | 'fail'
  /** Receives the timeout warning with `'continue'`. Default: console. */
  logger?: { warn(...args: any[]): void }
}

export interface StartResult {
  /** The first flag set from the server arrived in time. */
  ready: boolean
  /** Startup gave up waiting (`onTimeout: 'continue'`). */
  timedOut: boolean
}

export declare class CheckgateStartupTimeoutError extends Error {
  readonly name: 'CheckgateStartupTimeoutError'
  readonly timeoutMs: number
  /** The client's connection state when startup gave up. */
  readonly state: string
}

/** Connects `client`, waiting at most `readyTimeoutMs`; see {@link StartOptions}. */
export declare function startCheckgate(client: CheckgateClientLike, options?: StartOptions): Promise<StartResult>

/** Disconnects `client` once (until started again). Returns whether this call disconnected it. */
export declare function stopCheckgate(client: CheckgateClientLike): boolean

/** Per-flag test values: a value, or a function of the context. */
export type TestFlags = Record<string, FlagValue | ((context: { key: string; attributes: Record<string, string> }) => FlagValue)>

/** An in-memory client for application tests: no server, no native addon. */
export interface TestClient extends CheckgateClientLike {
  /** Calls to disconnect(). */
  readonly disconnectCalls: number
  /** Goal events recorded by track(). */
  readonly tracked: Array<{ eventKey: string; key: string; opts?: unknown }>
  getValue(flagKey: string, userKey: string, userAttributes?: Record<string, string>, defaultValue?: FlagValue): FlagValue
  isPolling(): boolean
  getStatus(): { state: 'idle' | 'streaming'; lastSyncAt: number | null; error: null }
  onChange(callback: (flagKey: string) => void): () => void
  track(eventKey: string, userKey: string, opts?: { value?: number; context?: Record<string, unknown> }): void
  /** Sets a flag's test value; `undefined` removes the flag. */
  setFlag(flagKey: string, value: TestFlags[string] | undefined): void
}

/**
 * `true`/`false`: on/off; another value: on, serving it; `null`: off; a key
 * not listed: missing.
 */
export declare function createTestClient(flags?: TestFlags): TestClient

/** Re-exported for convenience. */
export type { CheckgateClient }
