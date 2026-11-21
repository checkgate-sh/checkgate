import type {
  EvaluationContext,
  JsonValue,
  Logger,
  OpenFeatureEventEmitter,
  Provider,
  ResolutionDetails,
  TrackingEventDetails,
} from '@openfeature/server-sdk';
import type { CheckgateClient, CheckgateClientOptions } from '@checkgate/node';

interface ProviderBehaviour {
  /**
   * Stop waiting for the first flag set after this many milliseconds:
   * `initialize()` rejects (provider ERROR) while the client keeps connecting
   * in the background, and the provider turns READY once it succeeds.
   * Default: wait as long as it takes.
   */
  initTimeoutMs?: number;
  /**
   * Resolve `TARGETING_KEY_MISSING` when the context has no `targetingKey`.
   * With `false`, a missing key evaluates as the empty string. Default: true.
   */
  requireTargetingKey?: boolean;
}

/** Use an existing client (shared with direct SDK use); it is never disconnected by the provider. */
export interface CheckgateProviderClientOptions extends ProviderBehaviour {
  client: CheckgateClient;
}

/** Let the provider create, connect and (on close) disconnect its own client. */
export interface CheckgateProviderConnectionOptions extends ProviderBehaviour, CheckgateClientOptions {
  client?: undefined;
}

export type CheckgateProviderOptions =
  | CheckgateProviderClientOptions
  | CheckgateProviderConnectionOptions;

/**
 * OpenFeature provider (server paradigm) backed by Checkgate's local
 * evaluation engine. No network request per evaluation.
 *
 * Reasons: `DISABLED`, `TARGETING_MATCH`, `SPLIT` (partial rollout or
 * weighted variant, including users a rollout excluded), `DEFAULT`, `STATIC`,
 * and the custom `PREREQUISITE_FAILED`. `flagMetadata` carries
 * `checkgateReason` and, when they apply, `ruleIndex`, `variantIndex` and
 * `rolloutBucket`.
 */
export declare class CheckgateProvider implements Provider {
  constructor(options: CheckgateProviderOptions);
  readonly metadata: { readonly name: 'checkgate' };
  readonly runsOn: 'server';
  readonly hooks: [];
  readonly events: OpenFeatureEventEmitter;
  /** The Checkgate client answering evaluations. */
  readonly client: CheckgateClient;
  initialize(context?: EvaluationContext): Promise<void>;
  onClose(): Promise<void>;
  resolveBooleanEvaluation(flagKey: string, defaultValue: boolean, context: EvaluationContext, logger?: Logger): Promise<ResolutionDetails<boolean>>;
  resolveStringEvaluation(flagKey: string, defaultValue: string, context: EvaluationContext, logger?: Logger): Promise<ResolutionDetails<string>>;
  resolveNumberEvaluation(flagKey: string, defaultValue: number, context: EvaluationContext, logger?: Logger): Promise<ResolutionDetails<number>>;
  resolveObjectEvaluation<T extends JsonValue>(flagKey: string, defaultValue: T, context: EvaluationContext, logger?: Logger): Promise<ResolutionDetails<T>>;
  track(trackingEventName: string, context: EvaluationContext, trackingEventDetails?: TrackingEventDetails): void;
}

/** The Checkgate attributes an OpenFeature context becomes (strings only; `targetingKey` excluded). */
export declare function toAttributes(context: EvaluationContext | undefined): Record<string, string>;

/** Checkgate evaluation reason → OpenFeature reason. */
export declare const REASONS: Readonly<Record<string, string>>;
