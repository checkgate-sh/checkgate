export type Operator =
  | 'equals'
  | 'not_equals'
  | 'contains'
  | 'starts_with'
  | 'ends_with'
  | 'greater_than'
  | 'greater_than_or_equal'
  | 'less_than'
  | 'less_than_or_equal'

export type FlagType = 'boolean' | 'string' | 'integer' | 'json'

export type FlagValue = string | number | boolean | Record<string, unknown> | null

export interface TargetingRule {
  attribute: string
  operator: Operator
  values: string[]
  /** When set, this rule references a named segment instead of a concrete attribute check. */
  segment_key?: string
  variant?: FlagValue
}

export interface WeightedVariant {
  /** Relative weight — need not sum to 100 across all variants, only proportions matter. */
  weight: number
  value: FlagValue
}

export interface Prerequisite {
  flag_key: string
  /** Value the prerequisite flag must resolve to. Omit to just require it be enabled. */
  required_value?: FlagValue
}

export interface Flag {
  key: string
  is_enabled: boolean
  rollout_percentage: number | null
  description: string | null
  rules: TargetingRule[]
  flag_type?: FlagType
  default_value?: FlagValue
  disabled_value?: FlagValue
  /**
   * Weighted distribution across multiple variant values (e.g. a 60/30/10 A/B/C split).
   * When non-empty, an enabled evaluation that matches no targeting rule is bucketed across
   * these weighted variants instead of returning `default_value`.
   */
  variants?: WeightedVariant[]
  /**
   * Other flags this flag depends on. If any prerequisite is not satisfied, this flag
   * evaluates as disabled — even if its own `is_enabled`/rules/rollout would otherwise
   * say yes. Checked before rules and rollout.
   */
  prerequisites?: Prerequisite[]
  /** Free-form labels for search/filtering. Management metadata only — never sent to SDKs. */
  tags?: string[]
  /** Email of the person responsible for this flag. Management metadata only. */
  owner_email?: string | null
  /** RFC 3339 timestamp if archived (hidden from the default list), else absent. Purely a
   *  dashboard hygiene concept — does not affect evaluation. */
  archived_at?: string | null
}

export type FlagPatch = Partial<Omit<Flag, 'key'>>

export interface Impression {
  id: number
  flag_key: string
  user_id: string | null
  value: string
  context: Record<string, unknown> | null
  evaluated_at: string
}

export interface ImpressionListResponse {
  items: Impression[]
  total: number
}

export interface ImpressionStats {
  flag_key: string
  total: number
  true_count: number
  false_count: number
  unique_users: number
  last_seen: string | null
}

// --- Exposure dashboards ---------------------------------------------------

export interface ExposureVariant {
  value: string
  impressions: number
  unique_users: number
}

export interface ExposurePoint {
  day: string
  value: string
  count: number
}

export interface ExposureResponse {
  flag_key: string
  total_impressions: number
  total_users: number
  variants: ExposureVariant[]
  timeline: ExposurePoint[]
}

// --- Events & experiments (A/B testing) ------------------------------------

export interface EventKeyInfo {
  event_key: string
  total: number
  unique_users: number
  last_seen: string | null
}

export type ExperimentStatus = 'running' | 'paused' | 'completed'

export interface Experiment {
  id: string
  environment_id: string
  key: string
  name: string
  description: string | null
  flag_key: string
  goal_event_key: string
  control_variant: string | null
  status: ExperimentStatus
  created_at: string
}

export interface VariantResult {
  variant: string
  exposed: number
  converted: number
  conversion_rate: number
  is_control: boolean
  uplift: number | null
  z_score: number | null
  p_value: number | null
  significant: boolean
}

export interface ExperimentResults {
  experiment: Experiment
  control_variant: string | null
  total_exposed: number
  total_converted: number
  variants: VariantResult[]
}

export interface AuditEntry {
  id: number
  environment_id: string
  flag_key: string
  actor_email: string | null
  action: 'CREATE' | 'UPDATE' | 'DELETE' | 'PROMOTE' | 'ARCHIVE' | 'UNARCHIVE'
  before_data: Record<string, unknown> | null
  after_data: Record<string, unknown> | null
  metadata: Record<string, unknown> | null
  created_at: string
}

export interface Segment {
  id: string
  environment_id: string
  name: string
  key: string
  description: string | null
  rules: TargetingRule[]
  created_at: string
}

export type SegmentPatch = { name?: string; description?: string; rules?: TargetingRule[] }

export interface Webhook {
  id: string
  environment_id: string
  name: string
  url: string
  has_secret: boolean
  enabled: boolean
  created_at: string
}

export type WebhookPatch = { name?: string; url?: string; secret?: string; enabled?: boolean }

export interface WebhookDelivery {
  id: number
  webhook_id: string
  event: string
  status_code: number | null
  response_body: string | null
  error: string | null
  delivered_at: string
}

export interface ScheduledChange {
  id: string
  environment_id: string
  flag_key: string
  scheduled_at: string
  patch: Record<string, unknown>
  executed_at: string | null
  created_at: string
}

export interface ConnectedClient {
  connection_id: string
  environment_id: string | null
  sdk_key_name: string | null
  client_ip: string
  connected_at: number
}

export type UserRole = 'admin' | 'editor' | 'viewer'

export interface User {
  id: string
  email: string
  name: string
  role: UserRole
  createdAt: string
}

export interface Project {
  id: string
  name: string
  slug: string
  environment_count?: number
  member_count?: number
  created_at: string
}

export interface ProjectMember {
  user_id: number
  name: string
  email: string
  role: UserRole
}

export interface SdkKeyInfo {
  id: number
  name: string
  prefix: string
  environment_id: string
  environment_name: string
  created_at: string
}

// ---------------------------------------------------------------------------
// Chat integrations (Slack / Microsoft Teams)
// ---------------------------------------------------------------------------

export type IntegrationKind = 'slack' | 'teams'

/** Event names the server can notify on. Mirrors `SUPPORTED_EVENTS` in Rust. */
export const INTEGRATION_EVENTS = [
  'flag.created',
  'flag.updated',
  'flag.deleted',
  'flag.promoted',
  'change_request.opened',
  'change_request.approved',
  'change_request.rejected',
] as const

export interface Integration {
  id: string
  environment_id: string
  kind: IntegrationKind
  name: string
  /** Elided tail of the webhook URL — the full URL is never sent back. */
  webhook_url_preview: string
  /** Empty means "every event". */
  events: string[]
  enabled: boolean
  created_at: string
}

/** Result of `POST /integrations/{id}/test`: what the integration's receiver did. */
export interface IntegrationTestResult {
  /** The receiver answered 2xx. */
  delivered: boolean
  /** The receiver's HTTP status; `null` when it was never reached. */
  status_code: number | null
  /** Why nothing was delivered (never contains the URL); `null` when delivered. */
  error: string | null
}

export interface IntegrationDelivery {
  id: number
  integration_id: string
  event: string
  status_code: number | null
  response_body: string | null
  error: string | null
  delivered_at: string
}

// ---------------------------------------------------------------------------
// Evaluation playground (POST /api/environments/{env}/flags/{key}/evaluate)
// ---------------------------------------------------------------------------

export type EvaluationReason =
  | 'DISABLED'
  | 'PREREQUISITE_FAILED'
  | 'TARGETING_MATCH'
  | 'ROLLOUT_EXCLUDED'
  | 'SPLIT'
  | 'DEFAULT'
  | 'STATIC'

export interface EvaluationResult {
  enabled: boolean
  value: FlagValue
}

export interface PlaygroundPrerequisite {
  flag_key: string
  outcome: 'satisfied' | 'unsatisfied' | 'not_found' | 'limit_exceeded'
  required_value?: FlagValue
  result?: EvaluationResult
  revision?: number
}

export interface PlaygroundEvaluation extends EvaluationResult {
  environment_id: string
  flag_key: string
  flag_type: FlagType
  /** Revision of the flag configuration evaluated (what SDKs track). */
  revision: number
  reason: EvaluationReason
  matched_rule?: {
    rule: TargetingRule
    /** Index of the rule as stored (as the flag editor shows it). */
    source_index: number
    segment_key?: string
    segment_rule_index?: number
  }
  rollout?: { percentage: number; bucket: number }
  variant?: { index: number; weight: number; total_weight: number; value: FlagValue }
  prerequisites: PlaygroundPrerequisite[]
  referenced_attributes: string[]
  missing_attributes: string[]
}
