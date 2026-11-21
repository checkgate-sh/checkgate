/** Pluggable persistence adapter for offline flag evaluation. */
export interface CheckgateStorage {
  getItem(key: string): string | null | Promise<string | null>;
  setItem(key: string, value: string): void | Promise<void>;
}

/** A resolved multi-variant flag value: boolean, string, number, JSON object, or null. */
export type FlagValue = boolean | string | number | Record<string, unknown> | unknown[] | null;

/** Full evaluation result returned by {@link CheckgateClient.getVariant}. */
export interface FlagEvaluation {
  /** Whether the flag is enabled for this user. */
  enabled: boolean;
  /** The resolved variant value. */
  value: FlagValue;
}

/** Why an evaluation resolved as it did (see {@link CheckgateClient.getDetails}). */
export type EvaluationReason =
  | 'DISABLED'
  | 'PREREQUISITE_FAILED'
  | 'TARGETING_MATCH'
  | 'ROLLOUT_EXCLUDED'
  | 'SPLIT'
  | 'DEFAULT'
  | 'STATIC';

/** One prerequisite of the evaluated flag, in the order they were checked. */
export interface PrerequisiteCheck {
  flagKey: string;
  outcome: 'satisfied' | 'unsatisfied' | 'not_found' | 'limit_exceeded';
  /** The value the prerequisite must resolve to; absent means "enabled". */
  requiredValue?: FlagValue;
  /** What the prerequisite resolved to; absent when it was not evaluated. */
  result?: FlagEvaluation;
}

/** An evaluation plus the path that produced it. */
export interface EvaluationDetails extends FlagEvaluation {
  reason: EvaluationReason;
  /** Index into the flag's `rules` of the rule that matched. */
  ruleIndex?: number;
  /** The user's rollout bucket (0–99), when a partial rollout was checked. */
  rolloutBucket?: number;
  /** Index into the flag's `variants` of the weighted variant assigned. */
  variantIndex?: number;
  /** The flag's own prerequisites as checked (stops at the first failure). */
  prerequisites?: PrerequisiteCheck[];
}

export interface CheckgateClientOptions {
  /** Base URL of your Checkgate server. */
  serverUrl: string;
  /** SDK key for authentication. Leave unset for open (dev-only) mode. */
  sdkKey?: string;
  /** Base SSE reconnect delay in milliseconds; grows exponentially. Default: 3000. */
  reconnectDelayMs?: number;
  /** Maximum backoff delay between reconnect attempts, in milliseconds. Default: 30000. */
  maxReconnectDelayMs?: number;
  /**
   * Consecutive failed SSE reconnects before falling back to polling
   * `GET /flags/snapshot` (e.g. when a proxy blocks long-lived connections).
   * Set to `Infinity` to disable. Default: 3.
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
  /** Report evaluation events to the server for analytics. Default: true. */
  reportImpressions?: boolean;
  /** Flush buffered impressions once this many have accumulated. Default: 50. */
  impressionBatchSize?: number;
  /** Periodic impression flush interval in milliseconds. Default: 10000. */
  impressionFlushIntervalMs?: number;
  /**
   * Include user attributes as `context` in reported impressions. Default: false —
   * attributes never leave the process unless you explicitly opt in.
   */
  sendEvaluationContext?: boolean;
  /**
   * Optional persistence adapter. When provided, the last flag snapshot is saved
   * and re-hydrated on the next start so flags evaluate before/without a connection.
   */
  storage?: CheckgateStorage;
}

/** Connection states reported by {@link CheckgateClient.getStatus}. */
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
 * SDK diagnostics, schema 1 — the same fields in every Checkgate SDK (Python
 * uses snake_case). Local state only; never the SDK key, user keys or
 * attributes. Timestamps are milliseconds since the epoch.
 */
export interface CheckgateHealth {
  schemaVersion: 1;
  state: CheckgateConnectionState;
  /** A complete flag set from the server has been applied since connect(). */
  ready: boolean;
  /** Where the served flags came from. */
  source: 'none' | 'offline_cache' | 'server';
  flagCount: number;
  /** Last confirmation that the flags are current (as in getStatus()). */
  lastSyncAt: number | null;
  /** Milliseconds since lastSyncAt; `null` before the first sync. */
  configAgeMs: number | null;
  /** Highest flag revision seen in any event or snapshot, applied or not. */
  lastReceivedRevision: number | null;
  /** Highest revision in the flags currently held. */
  lastAppliedRevision: number | null;
  /** Events and snapshot entries rejected as not newer than what was held. */
  staleEventsIgnored: number;
  /** Consecutive failed attempts since the last successful connection. */
  reconnectAttempts: number;
  /** Reconnects scheduled since the client was created. */
  reconnectCount: number;
  /** Poll fallback active. */
  polling: boolean;
  /** Last successful poll, or `null`. */
  lastPollAt: number | null;
  /**
   * The most recent synchronization error. Not cleared by later success:
   * compare `at` with `lastSyncAt`.
   */
  lastError: { message: string; kind: 'auth' | 'network' | 'http' | 'parse' | 'timeout'; at: number } | null;
  offlineCache: { configured: boolean; hydrated: boolean; lastPersistAt: number | null };
}

/**
 * The server rejected the SDK key (HTTP 401 or 403). The client stops
 * retrying and keeps serving the flags it already has; call `connect()` again
 * to retry.
 */
export declare class CheckgateAuthError extends Error {
  readonly name: 'CheckgateAuthError';
  readonly status: number;
}

/**
 * Checkgate Node.js SDK client.
 *
 * Connects once to the server via SSE, downloads all flags, and evaluates
 * them locally, with no network IO, on every isEnabled() call.
 */
export declare class CheckgateClient {
  constructor(options: CheckgateClientOptions);

  /**
   * Connects to the server's SSE stream and downloads the current flag set.
   * Resolves once the server has replayed the full flag set and sent the
   * "ready" event. Sets up automatic reconnection on connection loss.
   *
   * Rejects with {@link CheckgateAuthError} if the server rejects the SDK key,
   * and with an `Error` if `disconnect()` is called first. While the server is
   * unreachable it keeps retrying.
   */
  connect(): Promise<void>;

  /** Connection health, for detecting stale flags. */
  getStatus(): CheckgateStatus;

  /** Detailed diagnostics (schema 1) for health checks and dashboards; no network IO. */
  getHealth(): CheckgateHealth;

  /**
   * Registers a listener called with {@link getStatus} whenever the connection
   * state changes, and after each poll (successful or not) while in poll fallback.
   *
   * @returns an unsubscribe function.
   */
  onStatusChange(callback: (status: CheckgateStatus) => void): () => void;

  /** Whether the initial bootstrap has completed and flags are ready to evaluate. */
  isReady(): boolean;

  /** Whether the client is currently polling `GET /flags/snapshot` instead of streaming. */
  isPolling(): boolean;

  /**
   * Registers a listener invoked whenever a flag changes after the initial
   * bootstrap (created, updated, or deleted). The flag's key is passed to the
   * callback. Bootstrap and reconnect re-syncs do not trigger listeners.
   *
   * @returns an unsubscribe function.
   */
  onChange(callback: (flagKey: string) => void): () => void;

  /**
   * Evaluates a flag for a user synchronously (no await, no network).
   *
   * @param flagKey      The flag key to evaluate.
   * @param userKey      Stable user identifier (used for rollout hashing).
   * @param userAttributes  User attributes for targeting rule matching.
   * @returns `true` if the flag is enabled for this user, `false` otherwise.
   *          Returns `false` if the flag does not exist.
   */
  isEnabled(
    flagKey: string,
    userKey: string,
    userAttributes?: Record<string, string>
  ): boolean;

  /**
   * Evaluates a multi-variant flag and returns the full result `{ enabled, value }`.
   *
   * @param flagKey        The flag key to evaluate.
   * @param userKey        Stable user identifier (used for rollout hashing).
   * @param userAttributes User attributes for targeting rule matching.
   * @returns The evaluation result, or `null` if the flag does not exist.
   */
  getVariant(
    flagKey: string,
    userKey: string,
    userAttributes?: Record<string, string>
  ): FlagEvaluation | null;

  /**
   * Evaluates a multi-variant flag and returns just its resolved value.
   *
   * @param flagKey        The flag key to evaluate.
   * @param userKey        Stable user identifier (used for rollout hashing).
   * @param userAttributes User attributes for targeting rule matching.
   * @param defaultValue   Returned when the flag does not exist. Defaults to `null`.
   */
  getValue(
    flagKey: string,
    userKey: string,
    userAttributes?: Record<string, string>,
    defaultValue?: FlagValue
  ): FlagValue;

  /**
   * Evaluates a flag and explains the result (reason, matched rule, rollout
   * bucket, weighted variant, prerequisites). Same local evaluation as
   * {@link getVariant}; no network IO.
   *
   * @returns The details, or `null` if the flag does not exist.
   */
  getDetails(
    flagKey: string,
    userKey: string,
    userAttributes?: Record<string, string>
  ): EvaluationDetails | null;

  /**
   * Records a goal/conversion event for A/B testing (e.g. "checkout_complete").
   * Buffered and reported asynchronously, mirroring impression reporting.
   *
   * @param eventKey The goal event name (must match the experiment's goal).
   * @param userKey  The same user identifier passed to getVariant()/isEnabled().
   * @param opts     Optional numeric `value` (e.g. revenue) and `context` metadata.
   */
  track(
    eventKey: string,
    userKey: string,
    opts?: { value?: number; context?: Record<string, unknown> }
  ): void;

  /**
   * Closes the SSE connection and cleans up resources.
   * Call on graceful shutdown.
   */
  disconnect(): void;
}
