/**
 * Persistence adapter for the last known flag set. `localStorage` satisfies
 * it, and so does any async key-value store.
 */
export interface CheckgateStorage {
  getItem(key: string): string | null | Promise<string | null>;
  setItem(key: string, value: string): void | Promise<void>;
}

/** A resolved multi-variant flag value: boolean, string, number, JSON object, or null. */
export type FlagValue = boolean | string | number | Record<string, unknown> | unknown[] | null;

/** Full evaluation result returned by {@link CheckgateWeb.getVariant}. */
export interface FlagEvaluation {
  /** Whether the flag is enabled for this user. */
  enabled: boolean;
  /** The resolved variant value. */
  value: FlagValue;
}

/** A stored flag as served by `/flags/snapshot` (and embedded by `@checkgate/ssr`). */
export type CheckgateFlag = { key: string } & Record<string, unknown>;

export interface CheckgateWebOptions {
  /** Base URL of your Checkgate server. */
  serverUrl: string;
  /**
   * SDK key. Browsers cannot set headers on `EventSource`, so the stream sends
   * it as a `?sdk_key=` query parameter; polls send it as `Authorization: Bearer`.
   * SDK keys are public by design. Leave unset for open (dev-only) mode.
   */
  sdkKey?: string;
  /** Base SSE reconnect delay in milliseconds; grows exponentially. Default: 3000. */
  reconnectDelayMs?: number;
  /** Maximum backoff delay between reconnect attempts, in milliseconds. Default: 30000. */
  maxReconnectDelayMs?: number;
  /**
   * Consecutive failed SSE reconnects before falling back to polling
   * `GET /flags/snapshot`. Set to `Infinity` to disable. Default: 3.
   */
  pollFallbackThreshold?: number;
  /** Poll interval in milliseconds while in fallback mode. Default: 30000. */
  pollIntervalMs?: number;
  /**
   * A stream attempt that has not received the server's `connected` event
   * within this many milliseconds is abandoned and retried. Default: 30000.
   */
  connectTimeoutMs?: number;
  /**
   * A connected stream with no event (heartbeats included) for this many
   * milliseconds is treated as dead and reconnected. Default: 3 × the heartbeat
   * interval the server announces (45 s with the default 15 s); no idle
   * timeout against servers older than 0.1.31. `0` disables it.
   */
  streamIdleTimeoutMs?: number;
  /**
   * Report evaluations (flag key, user key, result) to the server for
   * analytics. Default: true.
   */
  reportImpressions?: boolean;
  /** Flush buffered impressions once this many have accumulated. Default: 50. */
  impressionBatchSize?: number;
  /** Periodic impression flush interval in milliseconds. Default: 10000. */
  impressionFlushIntervalMs?: number;
  /**
   * Include user attributes as `context` in reported impressions. Default:
   * false, so attributes never leave the browser unless you opt in.
   */
  sendEvaluationContext?: boolean;
  /**
   * Where the last flag set is persisted and re-hydrated from on the next
   * start. Default: `localStorage` when available. Pass `null` to persist nothing.
   */
  storage?: CheckgateStorage | null;
  /**
   * A flag snapshot loaded when `connect()` is called, before the stream is
   * established: pass the payload (or its `flags`) from `@checkgate/ssr`.
   */
  bootstrap?: CheckgateFlag[] | { flags: CheckgateFlag[] } | null;
}

/** Connection states reported by {@link CheckgateWeb.getStatus}. */
export type CheckgateConnectionState =
  | 'idle'
  | 'connecting'
  | 'streaming'
  | 'reconnecting'
  | 'polling'
  | 'auth_failed'
  | 'closed';

export interface CheckgateStatus {
  state: CheckgateConnectionState;
  /**
   * When the client last confirmed its flags are current (a completed
   * bootstrap or poll, or any event on a live stream, heartbeats included), in
   * ms since the epoch; `null` before the first.
   */
  lastSyncAt: number | null;
  /** The error behind `'auth_failed'`, otherwise `null`. */
  error: Error | null;
}

/**
 * The server rejected the SDK key (HTTP 401 or 403). The client stops
 * retrying and keeps serving the flags it already has; call `connect()` again
 * to retry.
 */
export declare class CheckgateAuthError extends Error {
  constructor(status: number);
  readonly name: 'CheckgateAuthError';
  readonly status: number;
}

/**
 * Checkgate browser SDK. Connects once over SSE, keeps the flag set current,
 * and evaluates flags locally in WebAssembly: no request per decision.
 */
export declare class CheckgateWeb {
  constructor(options: CheckgateWebOptions);

  /**
   * Loads the WebAssembly core, connects to the SSE stream and resolves once
   * the full flag set has been received. Rejects with
   * {@link CheckgateAuthError} if the server rejects the SDK key, and with an
   * `Error` if `disconnect()` is called first. While the server is unreachable
   * it keeps retrying; race it with a timeout if the caller cannot wait.
   */
  connect(): Promise<void>;

  /** Connection health, for detecting stale flags. */
  getStatus(): CheckgateStatus;

  /** Whether the initial bootstrap has completed and flags are ready to evaluate. */
  isReady(): boolean;

  /** Whether the client is currently polling `GET /flags/snapshot` instead of streaming. */
  isPolling(): boolean;

  /**
   * Registers a listener invoked with the key of each flag that changes after
   * the initial bootstrap. A reconnect re-sync triggers it only for flags that
   * changed while disconnected.
   *
   * @returns an unsubscribe function.
   */
  onChange(callback: (flagKey: string) => void): () => void;

  /**
   * Evaluates a flag for a user synchronously. Returns `false` if the flag
   * does not exist, or before flags are loaded (`connect()`, storage or
   * `bootstrap`). Attributes that are not strings are ignored.
   */
  isEnabled(flagKey: string, userKey: string, userAttributes?: Record<string, string>): boolean;

  /**
   * Evaluates a multi-variant flag and returns `{ enabled, value }`, or `null`
   * if the flag does not exist or flags are not loaded yet.
   */
  getVariant(
    flagKey: string,
    userKey: string,
    userAttributes?: Record<string, string>
  ): FlagEvaluation | null;

  /**
   * Evaluates a multi-variant flag and returns just its value, or
   * `defaultValue` (default `null`) if the flag does not exist.
   */
  getValue(
    flagKey: string,
    userKey: string,
    userAttributes?: Record<string, string>,
    defaultValue?: FlagValue
  ): FlagValue;

  /**
   * Records a goal/conversion event for experiments (e.g. "checkout_complete").
   * Buffered and reported asynchronously.
   */
  track(
    eventKey: string,
    userKey: string,
    opts?: { value?: number; context?: Record<string, unknown> }
  ): void;

  /** Closes the stream, stops polling and flushes pending reports. */
  disconnect(): void;
}
