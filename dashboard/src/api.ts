import type {
  AuditEntry,
  ConnectedClient,
  EventKeyInfo,
  Experiment,
  ExperimentResults,
  ExperimentStatus,
  ExposureResponse,
  Flag,
  FlagPatch,
  ImpressionListResponse,
  ImpressionStats,
  Integration,
  IntegrationDelivery,
  IntegrationKind,
  ScheduledChange,
  Segment,
  SegmentPatch,
  Webhook,
  WebhookDelivery,
  WebhookPatch,
} from './types'

export interface ApiUser {
  id: number
  email: string
  name: string
  role: string
  created_at: string
}

function baseUrl(): string {
  return import.meta.env.VITE_API_URL ?? ''
}

async function requestRaw<T>(path: string, init?: RequestInit): Promise<{ status: number; body: T }> {
  const res = await fetch(`${baseUrl()}${path}`, {
    ...init,
    // `same-origin` ensures the HttpOnly session cookie is sent automatically.
    // Never `include` (would send cookies cross-origin) or `omit` (would break auth).
    credentials: 'same-origin',
    headers: {
      'Content-Type': 'application/json',
      // CSRF defence-in-depth header. Cross-origin requests cannot include this
      // header because CORS only allows Authorization and Content-Type — so the
      // server's CSRF middleware effectively blocks cross-origin mutations.
      'X-Checkgate-Request': 'true',
      ...init?.headers,
    },
  })
  if (!res.ok) {
    const text = await res.text().catch(() => '')
    try {
      const json = JSON.parse(text) as { error?: string }
      if (json.error) throw new Error(json.error)
    } catch (e) {
      if (e instanceof SyntaxError === false) throw e
    }
    throw new Error(text || res.statusText || `Error ${res.status}`)
  }
  if (res.status === 204) return { status: res.status, body: undefined as T }
  return { status: res.status, body: await res.json() as T }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  return (await requestRaw<T>(path, init)).body
}

/** Flags per list request — the server's maximum page size. */
const FLAG_PAGE_SIZE = 500
/** The server clamps deeper offsets, so paging past this would repeat a page. */
const MAX_FLAG_OFFSET = 1_000_000

export const api = {
  /**
   * Every flag in the environment. The server pages this list (200 flags per
   * page by default), so fetch until a short page — one request would silently
   * drop the rest from search, comparison, and dependency pickers.
   */
  async listFlags(
    envId: string,
    opts: { includeArchived?: boolean; tag?: string } = {},
  ): Promise<Flag[]> {
    const flags: Flag[] = []
    for (let offset = 0; offset <= MAX_FLAG_OFFSET; offset += FLAG_PAGE_SIZE) {
      const params = new URLSearchParams({ limit: String(FLAG_PAGE_SIZE), offset: String(offset) })
      if (opts.includeArchived) params.set('include_archived', 'true')
      if (opts.tag) params.set('tag', opts.tag)
      const page = await request<Flag[]>(`/api/environments/${envId}/flags?${params}`)
      flags.push(...page)
      if (page.length < FLAG_PAGE_SIZE) break
    }
    return flags
  },

  getFlag(envId: string, key: string): Promise<Flag> {
    return request(`/api/environments/${envId}/flags/${encodeURIComponent(key)}`)
  },

  createFlag(envId: string, flag: Flag): Promise<Flag> {
    return request(`/api/environments/${envId}/flags`, {
      method: 'POST',
      body: JSON.stringify(flag),
    })
  },

  // Returns { applied: true, flag } when the patch took effect immediately,
  // or { applied: false, changeRequest } when the environment requires
  // approval and the patch was queued instead (202 Accepted).
  async patchFlag(
    envId: string,
    key: string,
    patch: FlagPatch,
  ): Promise<{ applied: true; flag: Flag } | { applied: false; changeRequest: ChangeRequest }> {
    const { status, body } = await requestRaw<Flag | ChangeRequest>(
      `/api/environments/${envId}/flags/${encodeURIComponent(key)}`,
      { method: 'PATCH', body: JSON.stringify(patch) },
    )
    return status === 202
      ? { applied: false, changeRequest: body as ChangeRequest }
      : { applied: true, flag: body as Flag }
  },

  deleteFlag(envId: string, key: string): Promise<void> {
    return request(`/api/environments/${envId}/flags/${encodeURIComponent(key)}`, {
      method: 'DELETE',
    })
  },

  promoteFlag(envId: string, key: string, targetEnvId: string): Promise<Flag> {
    return request(`/api/environments/${envId}/flags/${encodeURIComponent(key)}/promote`, {
      method: 'POST',
      body: JSON.stringify({ target_env_id: targetEnvId }),
    })
  },

  archiveFlag(envId: string, key: string): Promise<Flag> {
    return request(`/api/environments/${envId}/flags/${encodeURIComponent(key)}/archive`, {
      method: 'POST',
    })
  },

  unarchiveFlag(envId: string, key: string): Promise<Flag> {
    return request(`/api/environments/${envId}/flags/${encodeURIComponent(key)}/unarchive`, {
      method: 'POST',
    })
  },

  listImpressions(
    envId: string,
    opts: {
      flagKey?: string
      userId?: string
      value?: string
      sinceId?: number
      limit?: number
      offset?: number
    } = {},
  ): Promise<ImpressionListResponse> {
    const params = new URLSearchParams()
    if (opts.flagKey) params.set('flag_key', opts.flagKey)
    if (opts.userId) params.set('user_id', opts.userId)
    if (opts.value) params.set('value', opts.value)
    if (opts.sinceId != null) params.set('since_id', String(opts.sinceId))
    if (opts.limit != null) params.set('limit', String(opts.limit))
    if (opts.offset != null) params.set('offset', String(opts.offset))
    const qs = params.toString()
    return request(`/api/environments/${envId}/impressions${qs ? `?${qs}` : ''}`)
  },

  impressionStats(envId: string): Promise<ImpressionStats[]> {
    return request(`/api/environments/${envId}/impressions/stats`)
  },

  exposure(envId: string, flagKey: string, days?: number): Promise<ExposureResponse> {
    const params = new URLSearchParams({ flag_key: flagKey })
    if (days != null) params.set('days', String(days))
    return request(`/api/environments/${envId}/impressions/exposure?${params.toString()}`)
  },
}

export const eventsApi = {
  keys(envId: string): Promise<EventKeyInfo[]> {
    return request(`/api/environments/${envId}/events/keys`)
  },
}

export const experimentsApi = {
  list(envId: string): Promise<Experiment[]> {
    return request(`/api/environments/${envId}/experiments`)
  },

  get(envId: string, key: string): Promise<Experiment> {
    return request(`/api/environments/${envId}/experiments/${encodeURIComponent(key)}`)
  },

  create(
    envId: string,
    data: {
      key: string
      name: string
      description?: string
      flag_key: string
      goal_event_key: string
      control_variant?: string | null
    },
  ): Promise<Experiment> {
    return request(`/api/environments/${envId}/experiments`, {
      method: 'POST',
      body: JSON.stringify(data),
    })
  },

  patch(
    envId: string,
    key: string,
    patch: {
      name?: string
      description?: string
      goal_event_key?: string
      control_variant?: string | null
      status?: ExperimentStatus
    },
  ): Promise<Experiment> {
    return request(`/api/environments/${envId}/experiments/${encodeURIComponent(key)}`, {
      method: 'PATCH',
      body: JSON.stringify(patch),
    })
  },

  delete(envId: string, key: string): Promise<void> {
    return request(`/api/environments/${envId}/experiments/${encodeURIComponent(key)}`, {
      method: 'DELETE',
    })
  },

  results(envId: string, key: string): Promise<ExperimentResults> {
    return request(`/api/environments/${envId}/experiments/${encodeURIComponent(key)}/results`)
  },
}

export const auditApi = {
  list(
    envId: string,
    opts: { flagKey?: string; limit?: number; offset?: number } = {},
  ): Promise<AuditEntry[]> {
    const params = new URLSearchParams()
    if (opts.flagKey) params.set('flag_key', opts.flagKey)
    if (opts.limit != null) params.set('limit', String(opts.limit))
    if (opts.offset != null) params.set('offset', String(opts.offset))
    const qs = params.toString()
    return request(`/api/environments/${envId}/audit${qs ? `?${qs}` : ''}`)
  },
}

export const segmentsApi = {
  list(envId: string): Promise<Segment[]> {
    return request(`/api/environments/${envId}/segments`)
  },

  get(envId: string, key: string): Promise<Segment> {
    return request(`/api/environments/${envId}/segments/${encodeURIComponent(key)}`)
  },

  create(
    envId: string,
    data: { name: string; key: string; description?: string; rules?: Segment['rules'] },
  ): Promise<Segment> {
    return request(`/api/environments/${envId}/segments`, {
      method: 'POST',
      body: JSON.stringify(data),
    })
  },

  patch(envId: string, key: string, patch: SegmentPatch): Promise<Segment> {
    return request(`/api/environments/${envId}/segments/${encodeURIComponent(key)}`, {
      method: 'PATCH',
      body: JSON.stringify(patch),
    })
  },

  delete(envId: string, key: string): Promise<void> {
    return request(`/api/environments/${envId}/segments/${encodeURIComponent(key)}`, {
      method: 'DELETE',
    })
  },
}

export const webhooksApi = {
  list(envId: string): Promise<Webhook[]> {
    return request(`/api/environments/${envId}/webhooks`)
  },

  create(
    envId: string,
    data: { name: string; url: string; secret?: string; enabled?: boolean },
  ): Promise<Webhook> {
    return request(`/api/environments/${envId}/webhooks`, {
      method: 'POST',
      body: JSON.stringify(data),
    })
  },

  patch(envId: string, id: string, patch: WebhookPatch): Promise<Webhook> {
    return request(`/api/environments/${envId}/webhooks/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(patch),
    })
  },

  delete(envId: string, id: string): Promise<void> {
    return request(`/api/environments/${envId}/webhooks/${id}`, { method: 'DELETE' })
  },

  listDeliveries(envId: string, webhookId: string): Promise<WebhookDelivery[]> {
    return request(`/api/environments/${envId}/webhooks/${webhookId}/deliveries`)
  },
}

export const scheduledApi = {
  list(envId: string): Promise<ScheduledChange[]> {
    return request(`/api/environments/${envId}/scheduled-changes`)
  },

  listForFlag(envId: string, flagKey: string): Promise<ScheduledChange[]> {
    return request(
      `/api/environments/${envId}/flags/${encodeURIComponent(flagKey)}/scheduled-changes`,
    )
  },

  create(
    envId: string,
    flagKey: string,
    data: { scheduled_at: string; patch: Record<string, unknown> },
  ): Promise<ScheduledChange> {
    return request(
      `/api/environments/${envId}/flags/${encodeURIComponent(flagKey)}/scheduled-changes`,
      {
        method: 'POST',
        body: JSON.stringify(data),
      },
    )
  },

  delete(envId: string, id: string): Promise<void> {
    return request(`/api/environments/${envId}/scheduled-changes/${id}`, { method: 'DELETE' })
  },
}

export const healthApi = {
  connections(): Promise<ConnectedClient[]> {
    return request('/api/health/connections')
  },
}

export const userApi = {
  list(): Promise<ApiUser[]> {
    return request('/api/users')
  },

  create(data: { name: string; email: string; role: string; password: string }): Promise<ApiUser> {
    return request('/api/users', {
      method: 'POST',
      body: JSON.stringify(data),
    })
  },

  remove(id: number): Promise<void> {
    return request(`/api/users/${id}`, { method: 'DELETE' })
  },
}

export interface SdkKeyInfo {
  id: number
  name: string
  prefix: string
  environment_id: string
  environment_name: string
  created_at: string
}

export interface NewKeyResponse {
  id: number
  name: string
  key: string
  prefix: string
  environment_id: string
  environment_name: string
  created_at: string
}

export const keysApi = {
  list(projectId: string): Promise<SdkKeyInfo[]> {
    return request(`/api/projects/${projectId}/keys`)
  },

  create(projectId: string, name: string, environmentId: string): Promise<NewKeyResponse> {
    return request(`/api/projects/${projectId}/keys`, {
      method: 'POST',
      body: JSON.stringify({ name, environment_id: environmentId }),
    })
  },

  revoke(projectId: string, id: number): Promise<void> {
    return request(`/api/projects/${projectId}/keys/${id}`, { method: 'DELETE' })
  },
}

export type TokenScope = 'read_only' | 'read_write'

export interface TokenInfo {
  id: number
  name: string
  prefix: string
  scope: TokenScope
  created_at: string
  last_used_at: string | null
  expires_at: string | null
}

export interface NewTokenResponse {
  id: number
  name: string
  token: string
  prefix: string
  scope: TokenScope
  created_at: string
  expires_at: string | null
}

export const tokensApi = {
  list(): Promise<TokenInfo[]> {
    return request('/api/tokens')
  },

  create(name: string, scope: TokenScope, expiresInDays: number | null): Promise<NewTokenResponse> {
    return request('/api/tokens', {
      method: 'POST',
      body: JSON.stringify({ name, scope, expires_in_days: expiresInDays }),
    })
  },

  revoke(id: number): Promise<void> {
    return request(`/api/tokens/${id}`, { method: 'DELETE' })
  },
}

export type ChangeRequestStatus = 'pending' | 'approved' | 'rejected' | 'cancelled'

export interface ChangeRequest {
  id: number
  environment_id: string
  flag_key: string
  patch: Record<string, unknown>
  requested_by: string
  status: ChangeRequestStatus
  reviewed_by: string | null
  reason: string | null
  created_at: string
  reviewed_at: string | null
}

export const changeRequestsApi = {
  list(envId: string, status?: ChangeRequestStatus): Promise<ChangeRequest[]> {
    const qs = status ? `?status=${status}` : ''
    return request(`/api/environments/${envId}/change-requests${qs}`)
  },

  approve(envId: string, id: number): Promise<Flag> {
    return request(`/api/environments/${envId}/change-requests/${id}/approve`, { method: 'POST' })
  },

  reject(envId: string, id: number, reason?: string): Promise<void> {
    return request(`/api/environments/${envId}/change-requests/${id}/reject`, {
      method: 'POST',
      body: JSON.stringify({ reason: reason ?? null }),
    })
  },

  cancel(envId: string, id: number): Promise<void> {
    return request(`/api/environments/${envId}/change-requests/${id}`, { method: 'DELETE' })
  },
}

export interface ProjectSummary {
  id: string
  name: string
  slug: string
  environment_count: number
  member_count: number
  created_at: string
}

export interface ProjectMemberInfo {
  user_id: number
  name: string
  email: string
  role: string
}

export const projectsApi = {
  list(): Promise<ProjectSummary[]> {
    return request('/api/projects')
  },

  create(name: string): Promise<ProjectSummary> {
    return request('/api/projects', { method: 'POST', body: JSON.stringify({ name }) })
  },

  rename(projectId: string, name: string): Promise<ProjectSummary> {
    return request(`/api/projects/${projectId}`, {
      method: 'PATCH',
      body: JSON.stringify({ name }),
    })
  },

  delete(projectId: string): Promise<void> {
    return request(`/api/projects/${projectId}`, { method: 'DELETE' })
  },

  listMembers(projectId: string): Promise<ProjectMemberInfo[]> {
    return request(`/api/projects/${projectId}/members`)
  },

  addMember(projectId: string, userId: number, role: string): Promise<ProjectMemberInfo> {
    return request(`/api/projects/${projectId}/members`, {
      method: 'POST',
      body: JSON.stringify({ user_id: userId, role }),
    })
  },

  updateMemberRole(projectId: string, userId: number, role: string): Promise<ProjectMemberInfo> {
    return request(`/api/projects/${projectId}/members/${userId}`, {
      method: 'PATCH',
      body: JSON.stringify({ role }),
    })
  },

  removeMember(projectId: string, userId: number): Promise<void> {
    return request(`/api/projects/${projectId}/members/${userId}`, { method: 'DELETE' })
  },
}

export const integrationsApi = {
  list(envId: string): Promise<Integration[]> {
    return request(`/api/environments/${envId}/integrations`)
  },

  create(
    envId: string,
    data: {
      kind: IntegrationKind
      name: string
      webhook_url: string
      events?: string[]
      enabled?: boolean
    },
  ): Promise<Integration> {
    return request(`/api/environments/${envId}/integrations`, {
      method: 'POST',
      body: JSON.stringify(data),
    })
  },

  patch(
    envId: string,
    id: string,
    patch: { name?: string; webhook_url?: string; events?: string[]; enabled?: boolean },
  ): Promise<Integration> {
    return request(`/api/environments/${envId}/integrations/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(patch),
    })
  },

  delete(envId: string, id: string): Promise<void> {
    return request(`/api/environments/${envId}/integrations/${id}`, { method: 'DELETE' })
  },

  /** Fires a sample message through the real delivery path. */
  test(envId: string, id: string): Promise<void> {
    return request(`/api/environments/${envId}/integrations/${id}/test`, { method: 'POST' })
  },

  listDeliveries(envId: string, id: string): Promise<IntegrationDelivery[]> {
    return request(`/api/environments/${envId}/integrations/${id}/deliveries`)
  },
}
