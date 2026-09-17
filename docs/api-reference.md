---
title: "REST API Reference — Checkgate Feature Flag Management API"
description: "Complete REST API reference for Checkgate. Manage projects, environments, flags, users, and SDK keys via CRUD endpoints. Connect to the SSE stream to push flag changes to SDK clients in under 50 ms."
---

# REST API Reference

## Base URL

```
http://your-checkgate-server:3000
```

## Authentication

All protected endpoints require one of:

```http
Authorization: Bearer sk_live_your_sdk_key
```

```http
Authorization: Bearer pat_your_personal_access_token
```

or an active session cookie set by `POST /api/auth/login`.

An SDK key is always admin-equivalent, intended for the SDKs themselves. A
personal access token (see [Personal Access Tokens](#personal-access-tokens))
instead acts *as its owning user* — same role, same project memberships —
and can be capped to read-only, making it the right choice for CI/CD,
Terraform, or any script that shouldn't hold blanket admin access.

Dashboard mutation requests also require:

```http
X-Checkgate-Request: true
```

This header is required for all state-changing requests from browser clients (CSRF protection). SDK clients using `Authorization: Bearer` are exempt.

---

## Auth

### Login

```http
POST /api/auth/login
Content-Type: application/json
```

**Request Body**

```json
{
  "email": "admin@example.com",
  "password": "your-password"
}
```

**Response** `200 OK`

```json
{
  "email": "admin@example.com",
  "name": "Juan Dela Cruz",
  "role": "admin",
  "workspace_name": "Acme Corp",
  "is_setup_complete": true
}
```

Sets an `HttpOnly` `SameSite=Strict` session cookie valid for 7 days. The server enforces the expiry and checks the account and its current role on every authenticated request. Deleting an account invalidates its existing sessions.

**Response** `401 Unauthorized`

```json
{
  "error": "Incorrect email or password.",
  "attempts_remaining": 4
}
```

**Response** `429 Too Many Requests` — account locked after 5 failures in 10 minutes.

```json
{
  "error": "Too many failed attempts. Account locked for 15 minutes.",
  "retry_after_seconds": 900
}
```

---

### Logout

```http
POST /api/auth/logout
```

Clears the session cookie.

**Response** `204 No Content`

---

### Get Current User

```http
GET /api/auth/me
```

**Response** `200 OK` — returns the authenticated user (same shape as login response, includes `is_setup_complete`).

**Response** `401 Unauthorized` — no valid session.

---

### Workspace Info

```http
GET /api/auth/workspace
```

Public endpoint — no auth required. Returns the workspace name for the login page.

**Response** `200 OK`

```json
{ "workspace_name": "Acme Corp" }
```

---

## Projects

Projects are the top-level namespace for environments, flags, and SDK keys. Workspace admins can manage all projects; other users see only projects they are members of.

### List Projects

```http
GET /api/projects
```

Returns all projects the caller has access to (all projects for workspace admins; only member projects for others).

**Response** `200 OK`

```json
[
  {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "name": "Mobile App",
    "slug": "mobile-app",
    "environment_count": 3,
    "member_count": 4,
    "created_at": "2026-04-18T00:00:00Z"
  }
]
```

---

### Create Project

```http
POST /api/projects
Content-Type: application/json
```

Admin only. Creates a new project and seeds it with Production, Staging, and Development environments plus one SDK key for the Production environment.

**Request Body**

```json
{ "name": "Mobile App" }
```

**Response** `200 OK` — returns the created project.

**Response** `409 Conflict` — slug derived from the name already exists.

---

### Rename Project

```http
PATCH /api/projects/{project_id}
Content-Type: application/json
```

```json
{ "name": "Mobile App v2" }
```

**Response** `200 OK` — returns the updated project.

---

### Delete Project

```http
DELETE /api/projects/{project_id}
```

Admin only. Cascades to all environments, flags, SDK keys, and impressions in the project.

**Response** `204 No Content`

**Response** `422 Unprocessable Entity` — cannot delete the last project.

---

## Project Members

### List Members

```http
GET /api/projects/{project_id}/members
```

**Response** `200 OK`

```json
[
  {
    "user_id": 1,
    "name": "Juan Dela Cruz",
    "email": "juan.delacruz@example.com",
    "role": "admin"
  }
]
```

---

### Add Member

```http
POST /api/projects/{project_id}/members
Content-Type: application/json
```

```json
{ "user_id": 2, "role": "editor" }
```

- `role` must be `"admin"`, `"editor"`, or `"viewer"`

**Response** `200 OK` — returns the added member.

**Response** `409 Conflict` — user is already a member.

---

### Update Member Role

```http
PATCH /api/projects/{project_id}/members/{user_id}
Content-Type: application/json
```

```json
{ "role": "viewer" }
```

**Response** `200 OK` — returns the updated member.

---

### Remove Member

```http
DELETE /api/projects/{project_id}/members/{user_id}
```

**Response** `204 No Content`

---

## Environments

Environments are scoped to a project. All environment routes are nested under `/api/projects/{project_id}/environments`.

### List Environments

```http
GET /api/projects/{project_id}/environments
```

**Response** `200 OK`

```json
[
  {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "name": "Production",
    "slug": "production",
    "color": "#ef4444",
    "is_default": false,
    "created_at": "2026-04-10T00:00:00Z"
  }
]
```

---

### Create Environment

```http
POST /api/projects/{project_id}/environments
Content-Type: application/json
```

**Request Body**

```json
{
  "name": "Canary",
  "slug": "canary",
  "color": "#f59e0b"
}
```

- `name` — required, max 100 characters
- `slug` — required, max 64 characters, lowercase alphanumerics and hyphens only
- `color` — optional, must be a valid hex color (`#rgb` or `#rrggbb`); defaults to `#6366f1`

**Response** `200 OK` — returns the created environment.

**Response** `409 Conflict` — slug already exists in this project.

---

### Update Environment

```http
PATCH /api/projects/{project_id}/environments/{env_id}
Content-Type: application/json
```

```json
{ "name": "Canary East", "color": "#8b5cf6" }
```

**Response** `200 OK` — returns the updated environment.

---

### Delete Environment

```http
DELETE /api/projects/{project_id}/environments/{env_id}
```

**Response** `204 No Content`

**Response** `422 Unprocessable Entity` — cannot delete the default or the last environment.

---

### Set Default Environment

```http
POST /api/projects/{project_id}/environments/{env_id}/default
```

**Response** `200 OK` — returns the updated environment with `is_default: true`.

---

### Set Approval Requirement

```http
POST /api/projects/{project_id}/environments/{env_id}/require-approval
Content-Type: application/json
```

```json
{ "require_approval": true }
```

When enabled, `PATCH` requests against flags in this environment no longer apply immediately — see [Partially Update a Flag](#partially-update-a-flag) and [Change Requests](#change-requests).

**Response** `200 OK` — returns the updated environment.

---

## Flags

Flag endpoints remain environment-scoped via `{env_id}` (UUID). The environment UUID can be obtained from the environments list.

### List Flags

```http
GET /api/environments/{env_id}/flags
```

**Response** `200 OK`

```json
[
  {
    "key": "new-checkout-flow",
    "is_enabled": true,
    "rollout_percentage": 50,
    "description": "New checkout UI rollout",
    "rules": []
  }
]
```

**Response** `404 Not Found` — environment does not exist.

---

### Get a Flag

```http
GET /api/environments/{env_id}/flags/{key}
```

**Response** `200 OK` — returns the flag object.

**Response** `404 Not Found` — flag or environment does not exist.

---

### Create or Replace a Flag

```http
POST /api/environments/{env_id}/flags
Content-Type: application/json
```

Creates a flag. If the key already exists in this environment, replaces it entirely unless the environment requires approval. In that case replacement returns `409 Conflict`; use PATCH to submit a change request.

**Request Body**

```json
{
  "key": "new-checkout-flow",
  "is_enabled": true,
  "rollout_percentage": 50,
  "description": "New checkout UI rollout",
  "rules": []
}
```

**Response** `200 OK` — returns the created flag.

**Response** `422 Unprocessable Entity` — invalid flag key or `rollout_percentage` out of range.

---

### Partially Update a Flag

```http
PATCH /api/environments/{env_id}/flags/{key}
Content-Type: application/json
```

Applies a JSON merge patch. Only the provided fields are updated; omitted fields retain their current values.

```json
{ "rollout_percentage": 100 }
```

```json
{ "is_enabled": false }
```

**Response** `200 OK` — returns the updated flag (applied immediately).

**Response** `202 Accepted` — the environment has `require_approval` set; the patch was validated and queued as a pending [change request](#change-requests) instead of applied. Returns the created change request, not the flag (which is unchanged).

**Response** `404 Not Found` — flag does not exist.

---

### Delete a Flag

```http
DELETE /api/environments/{env_id}/flags/{key}
```

Deletes the flag and broadcasts a `DELETE` event to connected SDK clients. Returns `409 Conflict` when the environment requires approval, preventing deletion and recreation from bypassing review.

**Response** `204 No Content`

---

### Promote a Flag

```http
POST /api/environments/{env_id}/flags/{key}/promote
Content-Type: application/json
```

Copies the flag's configuration from `{env_id}` to another environment atomically. The caller must have write access to the target project. Returns `409 Conflict` when the target environment requires approval; submit a PATCH there for review instead.

**Request Body**

```json
{ "target_env_id": "uuid-of-target-environment" }
```

**Response** `200 OK` — returns the flag as it now exists in the target environment.

---

## Segments

Segments are named, reusable groups of targeting rules scoped to an environment. A flag [targeting rule](#targetingrule-object) can reference a segment by `segment_key` instead of repeating the rules inline; the server expands the reference before flags reach SDK clients. Editing or deleting a segment automatically re-broadcasts every flag that references it. Segment writes return `409 Conflict` in environments that require approval, because they can change the behavior of existing flags. Segment keys follow the same constraints as flag keys (alphanumerics, underscores, hyphens; max 100 chars). Writes require **editor** role or above.

### List Segments

```http
GET /api/environments/{env_id}/segments
```

**Response** `200 OK`

```json
[
  {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "environment_id": "660e8400-e29b-41d4-a716-446655440001",
    "name": "Beta Users",
    "key": "beta-users",
    "description": "Internal beta cohort",
    "rules": [
      { "attribute": "plan", "operator": "equals", "values": ["beta"] }
    ],
    "created_at": "2026-07-04T00:00:00Z"
  }
]
```

---

### Get a Segment

```http
GET /api/environments/{env_id}/segments/{key}
```

**Response** `200 OK` — returns the segment object.

**Response** `404 Not Found` — segment does not exist.

---

### Create a Segment

```http
POST /api/environments/{env_id}/segments
Content-Type: application/json
```

Editor role or above.

**Request Body**

```json
{
  "name": "Beta Users",
  "key": "beta-users",
  "description": "Internal beta cohort",
  "rules": [
    { "attribute": "plan", "operator": "equals", "values": ["beta"] }
  ]
}
```

- `name` — required
- `key` — required, alphanumerics, underscores, hyphens; max 100 chars
- `description` — optional
- `rules` — array of [TargetingRule](#targetingrule-object) objects; defaults to `[]`

**Response** `200 OK` — returns the created segment.

**Response** `422 Unprocessable Entity` — invalid segment key.

---

### Partially Update a Segment

```http
PATCH /api/environments/{env_id}/segments/{key}
Content-Type: application/json
```

Editor role or above. Only the provided fields are updated; omitted fields retain their current values. Flags referencing this segment are re-broadcast to connected SDK clients.

```json
{ "rules": [ { "attribute": "plan", "operator": "equals", "values": ["beta", "trial"] } ] }
```

**Response** `200 OK` — returns the updated segment.

**Response** `404 Not Found` — segment does not exist.

---

### Delete a Segment

```http
DELETE /api/environments/{env_id}/segments/{key}
```

Editor role or above. Flags referencing this segment are re-broadcast without the segment's rules.

**Response** `204 No Content`

**Response** `404 Not Found` — segment does not exist.

---

## Change Requests

Only relevant for environments with [`require_approval`](#set-approval-requirement) enabled. A change
request captures a flag `PATCH` that hasn't been applied yet — see
[Partially Update a Flag](#partially-update-a-flag) for how one gets created. Someone other than the
requester must approve it (self-approval is rejected); admins may cancel anyone's pending request,
mirroring how workspace admins bypass project membership elsewhere.

### List Change Requests

```http
GET /api/environments/{env_id}/change-requests?status=pending
```

`status` is optional — one of `pending`, `approved`, `rejected`, `cancelled`. Omit to list all.

**Response** `200 OK`

```json
[
  {
    "id": 1,
    "environment_id": "550e8400-e29b-41d4-a716-446655440000",
    "flag_key": "dark_mode",
    "patch": { "is_enabled": true },
    "requested_by": "alice@acme.com",
    "status": "pending",
    "reviewed_by": null,
    "reason": null,
    "created_at": "2026-07-04T00:00:00Z",
    "reviewed_at": null
  }
]
```

---

### Approve Change Request

```http
POST /api/environments/{env_id}/change-requests/{id}/approve
```

Applies the stored patch to the flag's *current* state (not a stale snapshot from when the request was filed) and marks the request approved.

**Response** `200 OK` — returns the updated flag.

**Response** `403 Forbidden` — the caller is the original requester.

**Response** `409 Conflict` — the request is no longer pending (already approved/rejected/cancelled).

---

### Reject Change Request

```http
POST /api/environments/{env_id}/change-requests/{id}/reject
Content-Type: application/json
```

```json
{ "reason": "Not ready for this rollout yet" }
```

`reason` is optional. The flag is never touched.

**Response** `204 No Content`

---

### Cancel (Withdraw) Change Request

```http
DELETE /api/environments/{env_id}/change-requests/{id}
```

Only the original requester or a workspace admin may cancel, and only while still `pending`.

**Response** `204 No Content`

**Response** `404 Not Found` — no such pending request, or the caller doesn't own it.

---

## Flag Schema

### Flag Object

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `key` | `string` | Yes | Unique within environment. Alphanumerics, underscores, hyphens. Max 100 chars. Immutable after creation. |
| `is_enabled` | `boolean` | Yes | Master switch. `false` always evaluates to the disabled result. |
| `rollout_percentage` | `integer \| null` | No | 0–100. `null` is treated as 100%. |
| `description` | `string \| null` | No | Human-readable description. |
| `rules` | `TargetingRule[]` | No | Targeting rules, evaluated in order (first match wins). Defaults to `[]`. |
| `flag_type` | `"boolean" \| "string" \| "integer" \| "json"` | No | Variant type. Defaults to `"boolean"`. |
| `default_value` | any (matching `flag_type`) | No | Returned when enabled, inside the rollout, and no rule matched. |
| `disabled_value` | any (matching `flag_type`) | No | Returned when disabled, outside the rollout, or a prerequisite fails. |
| `variants` | `WeightedVariant[]` | No | Weighted multivariate distribution — see [Weighted Variants](guide/concepts.md#weighted-variants-a-b-testing). Defaults to `[]`. |
| `prerequisites` | `Prerequisite[]` | No | Other flags this flag depends on — see [Prerequisite Flags](guide/concepts.md#prerequisite-flags). Defaults to `[]`. |
| `tags` | `string[]` | No | Free-form lifecycle/organization tags. Defaults to `[]`. |
| `owner_email` | `string \| null` | No | Email of the flag's owner. |
| `archived_at` | ISO-8601 string \| null | No | Set when the flag is archived; `null` for active flags. |

### TargetingRule Object

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `attribute` | `string` | Yes* | User attribute name to match against. Not required when `segment_key` is set. |
| `operator` | `Operator` | Yes* | Comparison operator. Not required when `segment_key` is set. |
| `values` | `string[]` | Yes* | One or more values to match. At least one must match. Not required when `segment_key` is set. |
| `segment_key` | `string \| null` | No | References a named segment instead of a concrete attribute check. Expanded server-side before reaching SDKs. |
| `variant` | any (matching `flag_type`) | No | Value returned when this rule matches (non-boolean flags). Falls back to `default_value` if unset. |

### Operator Enum

| Value | Description |
|-------|-------------|
| `equals` | Attribute value equals any of the provided values |
| `not_equals` | Attribute value does not equal any of the provided values |
| `contains` | Attribute value contains any of the provided values as a substring |
| `starts_with` | Attribute value starts with any of the provided values |
| `ends_with` | Attribute value ends with any of the provided values |
| `greater_than` | Attribute, parsed as a number, is greater than any of the provided values |
| `greater_than_or_equal` | Attribute, parsed as a number, is greater than or equal to any of the provided values |
| `less_than` | Attribute, parsed as a number, is less than any of the provided values |
| `less_than_or_equal` | Attribute, parsed as a number, is less than or equal to any of the provided values |

Non-numeric attribute or rule values never match the four numeric operators (no error — the
comparison is simply skipped).

### WeightedVariant Object

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `weight` | `integer` | Yes | Relative weight. Weights need not sum to 100 — normalized against their total. |
| `value` | any (matching `flag_type`) | Yes | The variant value returned when this weighted bucket is selected. |

### Prerequisite Object

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `flag_key` | `string` | Yes | The prerequisite flag's key. |
| `required_value` | any (matching the prerequisite's `flag_type`) | No | The value the prerequisite must resolve to. Omit to just require it be enabled. |

---

## Impressions

### Ingest Impressions

```http
POST /api/environments/{env_id}/impressions
Authorization: Bearer sk_live_your_key
Content-Type: application/json
```

Reports a batch of flag evaluation events from an SDK client. Authenticated with an SDK key; does not require admin role. CSRF header not required for Bearer-authenticated requests.

**Request Body** — array of up to 500 impression objects

```json
[
  {
    "flag_key": "checkout_v2",
    "user_id": "user-123",
    "value": "true",
    "context": { "plan": "pro" },
    "evaluated_at": "2026-04-18T10:00:00Z"
  }
]
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `flag_key` | `string` | Yes | The evaluated flag key |
| `user_id` | `string \| null` | No | User identifier; `null` for anonymous |
| `value` | `string` | Yes | Evaluation result (e.g. `"true"`, `"false"`) |
| `context` | `object \| null` | No | Evaluation context attributes |
| `evaluated_at` | ISO-8601 string | No | Defaults to server receive time |

**Response** `204 No Content`

**Response** `413 Payload Too Large` — batch exceeds 500 items.

---

### List Impressions

```http
GET /api/environments/{env_id}/impressions
```

**Query Parameters**

| Param | Default | Description |
|-------|---------|-------------|
| `flag_key` | — | Filter by exact flag key |
| `user_id` | — | Filter by exact user ID |
| `value` | — | Filter by evaluated value (e.g. `"true"`, `"false"`) |
| `since_id` | — | Return only rows with `id > since_id` — for live polling |
| `limit` | `50` | Max results (1–200) |
| `offset` | `0` | Pagination offset (ignored when `since_id` is set) |

`total` in the response always reflects the count matching the base filters (ignoring `since_id`), so it can be used for pagination display regardless of polling state.

**Response** `200 OK`

```json
{
  "items": [
    {
      "id": 142,
      "flag_key": "checkout_v2",
      "user_id": "user-123",
      "value": "true",
      "context": { "plan": "pro" },
      "evaluated_at": "2026-04-18T10:00:00Z"
    }
  ],
  "total": 142
}
```

---

### Impression Stats

```http
GET /api/environments/{env_id}/impressions/stats
```

Returns per-flag aggregate counts.

**Response** `200 OK`

```json
[
  {
    "flag_key": "checkout_v2",
    "total": 1420,
    "true_count": 750,
    "false_count": 670,
    "unique_users": 89,
    "last_seen": "2026-04-18T10:30:00Z"
  }
]
```

---

### Exposure

```http
GET /api/environments/{env_id}/impressions/exposure?flag_key=checkout_v2&days=14
```

Exposure breakdown for a single flag — per-variant impression and unique-user counts across the whole retained window, plus a daily timeline of evaluations per variant. Powers the "which users are being exposed to which variant?" dashboard.

**Query Parameters**

| Param | Default | Description |
|-------|---------|-------------|
| `flag_key` | — | Required. The flag to break down (max 100 chars). |
| `days` | `14` | Trailing days included in the daily timeline (clamped to 1–90). |

**Response** `200 OK`

```json
{
  "flag_key": "checkout_v2",
  "total_impressions": 1420,
  "total_users": 89,
  "variants": [
    { "value": "true", "impressions": 750, "unique_users": 60 },
    { "value": "false", "impressions": 670, "unique_users": 52 }
  ],
  "timeline": [
    { "day": "2026-07-01", "value": "true", "count": 42 },
    { "day": "2026-07-01", "value": "false", "count": 38 }
  ]
}
```

**Response** `422 Unprocessable Entity` — `flag_key` missing or too long.

---

## Experiments

Experiments pair a flag with a goal event to measure conversion per variant. They are environment-scoped. Each experiment names the `flag_key` whose variants are the treatment arms and the `goal_event_key` (see [Events](#events-a-b-goal-events)) that counts as a conversion. Writes require **editor** role or above.

### List Experiments

```http
GET /api/environments/{env_id}/experiments
```

**Response** `200 OK`

```json
[
  {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "environment_id": "660e8400-e29b-41d4-a716-446655440001",
    "key": "checkout-copy-test",
    "name": "Checkout copy test",
    "description": "Does the new CTA convert better?",
    "flag_key": "checkout_v2",
    "goal_event_key": "purchase_completed",
    "control_variant": "false",
    "status": "running",
    "created_at": "2026-07-04T00:00:00Z"
  }
]
```

---

### Get an Experiment

```http
GET /api/environments/{env_id}/experiments/{key}
```

**Response** `200 OK` — returns the experiment object.

**Response** `404 Not Found` — experiment does not exist.

---

### Create an Experiment

```http
POST /api/environments/{env_id}/experiments
Content-Type: application/json
```

Editor role or above.

**Request Body**

```json
{
  "name": "Checkout copy test",
  "key": "checkout-copy-test",
  "description": "Does the new CTA convert better?",
  "flag_key": "checkout_v2",
  "goal_event_key": "purchase_completed",
  "control_variant": "false"
}
```

- `name` — required, non-empty
- `key` — required, alphanumerics, underscores, hyphens; max 100 chars
- `flag_key` — required, the flag under test (same key constraints)
- `goal_event_key` — required, the conversion event key (same key constraints)
- `control_variant` — optional, the baseline variant to compare against
- `description` — optional

**Response** `200 OK` — returns the created experiment.

**Response** `409 Conflict` — an experiment with this key already exists in the environment.

**Response** `422 Unprocessable Entity` — invalid key, flag key, goal event key, or empty name.

---

### Partially Update an Experiment

```http
PATCH /api/environments/{env_id}/experiments/{key}
Content-Type: application/json
```

Editor role or above. Only the provided fields are updated.

```json
{ "status": "paused" }
```

- `status` — one of `running`, `paused`, `completed`
- `name`, `description`, `goal_event_key`, `control_variant` — also updatable (`goal_event_key` must satisfy the key constraints)

**Response** `200 OK` — returns the updated experiment.

**Response** `404 Not Found` — experiment does not exist.

**Response** `422 Unprocessable Entity` — invalid `status` or `goal_event_key`.

---

### Delete an Experiment

```http
DELETE /api/environments/{env_id}/experiments/{key}
```

Editor role or above.

**Response** `204 No Content`

**Response** `404 Not Found` — experiment does not exist.

---

### Experiment Results

```http
GET /api/environments/{env_id}/experiments/{key}/results
```

Computes per-variant conversion rates and a two-proportion significance test of each variant against the control. Each user enters the experiment at their **first** exposure to the flag and is bucketed into the variant they saw then; a user counts as converted only if they fired the goal event at or after that first-exposure timestamp. The control is the experiment's `control_variant` when present in the data, otherwise the highest-exposure variant.

**Response** `200 OK`

```json
{
  "experiment": {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "environment_id": "660e8400-e29b-41d4-a716-446655440001",
    "key": "checkout-copy-test",
    "name": "Checkout copy test",
    "description": "Does the new CTA convert better?",
    "flag_key": "checkout_v2",
    "goal_event_key": "purchase_completed",
    "control_variant": "false",
    "status": "running",
    "created_at": "2026-07-04T00:00:00Z"
  },
  "control_variant": "false",
  "total_exposed": 1200,
  "total_converted": 240,
  "variants": [
    {
      "variant": "false",
      "exposed": 600,
      "converted": 90,
      "conversion_rate": 0.15,
      "is_control": true,
      "uplift": null,
      "z_score": null,
      "p_value": null,
      "significant": false
    },
    {
      "variant": "true",
      "exposed": 600,
      "converted": 150,
      "conversion_rate": 0.25,
      "is_control": false,
      "uplift": 0.6667,
      "z_score": 4.33,
      "p_value": 0.0000149,
      "significant": true
    }
  ]
}
```

| Field (per variant) | Type | Description |
|---------------------|------|-------------|
| `variant` | `string` | The evaluated value that defines this arm. |
| `exposed` | `integer` | Distinct users bucketed into this variant (by first exposure). |
| `converted` | `integer` | Of those, how many fired the goal event at/after exposure. |
| `conversion_rate` | `number` | `converted / exposed`, in `[0, 1]`. |
| `is_control` | `boolean` | `true` for the baseline variant the others compare against. |
| `uplift` | `number \| null` | Relative uplift vs. control (e.g. `0.12` = +12%). `null` for the control or when control conversion is zero. |
| `z_score` | `number \| null` | Two-proportion z-score vs. control. `null` for the control or when the test is undefined. |
| `p_value` | `number \| null` | Two-sided p-value for the z-score. `null` when `z_score` is `null`. |
| `significant` | `boolean` | `true` when `p_value < 0.05` (95% gate). |

**Response** `404 Not Found` — experiment does not exist.

---

## Events (A/B Goal Events)

Goal (conversion) events reported by SDK clients via `track()`. They are the denominator-matched conversions used by [experiment results](#experiment-results). Ingest is fire-and-forget, mirroring [impression ingest](#ingest-impressions).

### Ingest Events

```http
POST /api/environments/{env_id}/events
Authorization: Bearer sk_live_your_key
Content-Type: application/json
```

Authenticated with an SDK key; does not require admin role. CSRF header not required for Bearer-authenticated requests.

**Request Body** — array of up to 500 event objects

```json
[
  {
    "event_key": "purchase_completed",
    "user_id": "user-123",
    "value": 49.99,
    "context": { "plan": "pro" },
    "occurred_at": "2026-04-18T10:00:00Z"
  }
]
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `event_key` | `string` | Yes | Goal event identifier (max 100 chars; rows with an empty or over-long key are skipped) |
| `user_id` | `string \| null` | No | User identifier; `null` for anonymous |
| `value` | `number \| null` | No | Optional numeric payload (revenue, count, duration…) |
| `context` | `object \| null` | No | Arbitrary context attributes |
| `occurred_at` | ISO-8601 string | No | Defaults to server receive time; clamped to the server clock so a conversion never sorts ahead of its exposure |

**Response** `204 No Content`

**Response** `404 Not Found` — environment does not exist.

**Response** `413 Payload Too Large` — batch exceeds 500 items.

---

### List Event Keys

```http
GET /api/environments/{env_id}/events/keys
```

Returns the distinct goal-event keys seen in this environment, each with a usage count — used to populate the goal-event picker when creating an experiment.

**Response** `200 OK`

```json
[
  {
    "event_key": "purchase_completed",
    "total": 1240,
    "unique_users": 310,
    "last_seen": "2026-04-18T10:30:00Z"
  }
]
```

---

## SDK Keys

SDK keys are scoped to a project and bound to a specific environment. All key routes are nested under `/api/projects/{project_id}/keys`.

### List Keys

```http
GET /api/projects/{project_id}/keys
```

Returns all SDK keys for the project. The full key value is never returned after creation.

**Response** `200 OK`

```json
[
  {
    "id": 1,
    "name": "Default",
    "prefix": "sk_live_a1b2c3…",
    "environment_id": "550e8400-e29b-41d4-a716-446655440000",
    "environment_name": "Production",
    "created_at": "2026-04-10T00:00:00Z"
  }
]
```

---

### Create Key

```http
POST /api/projects/{project_id}/keys
Content-Type: application/json
```

```json
{
  "name": "iOS Production",
  "environment_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

- `environment_id` must belong to the project

**Response** `200 OK` — returns the key including the full value (shown once only).

```json
{
  "id": 2,
  "name": "iOS Production",
  "key": "sk_live_a1b2c3d4e5f6...",
  "prefix": "sk_live_a1b2c3…",
  "environment_id": "550e8400-e29b-41d4-a716-446655440000",
  "environment_name": "Production",
  "created_at": "2026-04-18T12:00:00Z"
}
```

---

### Revoke Key

```http
DELETE /api/projects/{project_id}/keys/{id}
```

**Response** `204 No Content`

**Response** `422 Unprocessable Entity` — cannot revoke the last key in the project.

---

## Personal Access Tokens

Personal access tokens are user-owned, not project-scoped — routes are flat
under `/api/tokens` and always operate on the caller's own tokens. There is
no admin override; revoking a departing or compromised user's automation
access is done by deleting the user, which cascades to their tokens.

### List Tokens

```http
GET /api/tokens
```

Returns the caller's own tokens. The full token value is never returned after creation.

**Response** `200 OK`

```json
[
  {
    "id": 1,
    "name": "Terraform CI",
    "prefix": "pat_a1b2c3d4e5f6…",
    "scope": "read_only",
    "created_at": "2026-07-04T00:00:00Z",
    "last_used_at": "2026-07-04T09:02:09Z",
    "expires_at": "2026-08-03T00:00:00Z"
  }
]
```

---

### Create Token

```http
POST /api/tokens
Content-Type: application/json
```

```json
{
  "name": "Terraform CI",
  "scope": "read_only",
  "expires_in_days": 90
}
```

- `scope` is `"read_only"` or `"read_write"`. A token authenticated request that is itself `read_only` cannot create a `read_write` token — this would let a leaked read-only credential mint itself a more privileged replacement.
- `expires_in_days` is optional (1–365); omit for a token that never expires.

**Response** `200 OK` — returns the token including the full value (shown once only).

```json
{
  "id": 2,
  "name": "Terraform CI",
  "token": "pat_a1b2c3d4e5f6...",
  "prefix": "pat_a1b2c3d4e5f6…",
  "scope": "read_only",
  "created_at": "2026-07-04T00:00:00Z",
  "expires_at": "2026-10-02T00:00:00Z"
}
```

**Response** `403 Forbidden` — a read-only-scoped caller tried to create a read-write token.

---

### Revoke Token

```http
DELETE /api/tokens/{id}
```

**Response** `204 No Content`

**Response** `404 Not Found` — no such token owned by the caller.

---

## Users

Users are workspace-level. Role here is the workspace role; per-project access is managed via Project Members.

### List Users

```http
GET /api/users
```

**Response** `200 OK`

```json
[
  {
    "id": 1,
    "name": "Juan Dela Cruz",
    "email": "juan.delacruz@example.com",
    "role": "admin",
    "created_at": "2026-04-10T00:00:00Z"
  }
]
```

---

### Create User

```http
POST /api/users
Content-Type: application/json
```

```json
{
  "name": "Bob Jones",
  "email": "bob@example.com",
  "role": "viewer",
  "password": "their-initial-password"
}
```

- `role` must be `"admin"`, `"editor"`, or `"viewer"`
- `password` must be at least 8 characters
- `email` must be unique

**Response** `200 OK` — returns the created user (password hash is never returned).

**Response** `409 Conflict` — email already in use.

```json
{ "error": "Email address is already in use." }
```

**Response** `422 Unprocessable Entity` — invalid input or password too short.

---

### Delete User

```http
DELETE /api/users/{id}
```

**Response** `204 No Content`

**Response** `422 Unprocessable Entity` — cannot delete yourself or the last admin.

---

## Scheduled Changes

A scheduled change stores a flag `PATCH` to be applied automatically at a future time. Each is bound to a specific flag and environment; once applied, its `executed_at` is set and it can no longer be deleted. Writes require **editor** role or above.

### List Scheduled Changes

```http
GET /api/environments/{env_id}/scheduled-changes
```

Lists all scheduled changes in the environment, ordered by `scheduled_at`. Each response includes `attempts` and `last_error`. Failed changes remain pending and are retried after at least 60 seconds; a successful retry clears `last_error`. If approval is enabled after scheduling, the worker retains the change without applying it until the policy permits execution.

**Response** `200 OK`

```json
[
  {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "environment_id": "660e8400-e29b-41d4-a716-446655440001",
    "flag_key": "checkout_v2",
    "scheduled_at": "2026-07-20T09:00:00Z",
    "patch": { "is_enabled": true },
    "executed_at": null,
    "attempts": 0,
    "last_error": null,
    "created_at": "2026-07-04T00:00:00Z"
  }
]
```

---

### List Scheduled Changes for a Flag

```http
GET /api/environments/{env_id}/flags/{key}/scheduled-changes
```

Same shape as above, filtered to a single flag.

**Response** `200 OK` — array of scheduled changes for the flag.

---

### Create a Scheduled Change

```http
POST /api/environments/{env_id}/flags/{key}/scheduled-changes
Content-Type: application/json
```

Editor role or above. The target flag must already exist.

**Request Body**

```json
{
  "scheduled_at": "2026-07-20T09:00:00Z",
  "patch": { "is_enabled": true }
}
```

- `scheduled_at` — required, RFC-3339 / ISO-8601 timestamp; more than 30 seconds in the past is rejected
- `patch` — required, a JSON object validated against the current flag before scheduling

Returns `422 Unprocessable Entity` for an invalid patch or timestamp, and `409 Conflict` if the environment requires approval.

**Response** `200 OK` — returns the created scheduled change.

**Response** `404 Not Found` — the flag does not exist.

---

### Delete a Scheduled Change

```http
DELETE /api/environments/{env_id}/scheduled-changes/{id}
```

Editor role or above. Only changes that have not yet executed can be deleted.

**Response** `204 No Content`

**Response** `404 Not Found` — no such pending change, or it has already executed.

---

## Webhooks

Webhooks POST a JSON payload to an external URL when flags change in an environment. Delivery attempts are recorded in a per-webhook delivery log. All webhook routes require **admin** role. The `secret` is never returned after creation — responses expose only a `has_secret` boolean.

### List Webhooks

```http
GET /api/environments/{env_id}/webhooks
```

**Response** `200 OK`

```json
[
  {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "environment_id": "660e8400-e29b-41d4-a716-446655440001",
    "name": "Slack notifier",
    "url": "https://hooks.example.com/flag-changes",
    "has_secret": true,
    "enabled": true,
    "created_at": "2026-07-04T00:00:00Z"
  }
]
```

---

### Create a Webhook

```http
POST /api/environments/{env_id}/webhooks
Content-Type: application/json
```

Admin only.

**Request Body**

```json
{
  "name": "Slack notifier",
  "url": "https://hooks.example.com/flag-changes",
  "secret": "whsec_your_signing_secret",
  "enabled": true
}
```

- `name` — required
- `url` — required, non-empty
- `secret` — optional; used to sign delivery payloads. Never returned afterwards.
- `enabled` — optional, defaults to `true`

**Response** `200 OK` — returns the created webhook (with `has_secret`, not the secret).

**Response** `422 Unprocessable Entity` — empty `url`.

---

### Update a Webhook

```http
PATCH /api/environments/{env_id}/webhooks/{id}
Content-Type: application/json
```

Admin only. Only the provided fields are updated.

```json
{ "enabled": false }
```

**Response** `200 OK` — returns the updated webhook.

**Response** `404 Not Found` — webhook does not exist in this environment.

---

### Delete a Webhook

```http
DELETE /api/environments/{env_id}/webhooks/{id}
```

Admin only.

**Response** `204 No Content`

**Response** `404 Not Found` — webhook does not exist in this environment.

---

### List Webhook Deliveries

```http
GET /api/environments/{env_id}/webhooks/{id}/deliveries
```

Returns the 100 most recent delivery attempts for the webhook, newest first.

**Response** `200 OK`

```json
[
  {
    "id": 42,
    "webhook_id": "550e8400-e29b-41d4-a716-446655440000",
    "event": "flag.updated",
    "status_code": 200,
    "response_body": "ok",
    "error": null,
    "delivered_at": "2026-07-04T10:00:00Z"
  }
]
```

- `status_code` / `response_body` are `null` when the request never completed; `error` then holds the failure reason.

**Response** `404 Not Found` — webhook does not exist in this environment.

---

## Audit Log

An append-only record of flag changes per environment. Each entry captures the actor, action, and before/after flag state.

### List Audit Log

```http
GET /api/environments/{env_id}/audit?flag_key=checkout_v2&limit=50&offset=0
```

**Query Parameters**

| Param | Default | Description |
|-------|---------|-------------|
| `flag_key` | — | Filter to a single flag |
| `limit` | `50` | Max results |
| `offset` | `0` | Pagination offset |

Entries are returned newest first.

**Response** `200 OK`

```json
[
  {
    "id": 128,
    "environment_id": "660e8400-e29b-41d4-a716-446655440001",
    "flag_key": "checkout_v2",
    "actor_email": "juan.delacruz@example.com",
    "action": "update",
    "before_data": { "is_enabled": false },
    "after_data": { "is_enabled": true },
    "metadata": null,
    "created_at": "2026-07-04T10:00:00Z"
  }
]
```

---

## SSE Stream

### Connect

```http
GET /stream
Authorization: Bearer sk_live_your_key
Accept: text/event-stream
```

Or for browser `EventSource`:

```
GET /stream?sdk_key=sk_live_your_key
```

Opens a persistent SSE connection scoped to the environment associated with the SDK key. The server sends:

1. A `connected` event immediately
2. One `update` event per existing flag in the key's environment (bootstrap)
3. A `ready` event once the bootstrap replay is complete
4. `update` events as flags change in that environment in real time
5. Keep-alive comments every 15 seconds

### Event: `connected`

```
event: connected
data: {"environment_id":"<uuid>"}
```

Sent once when the connection is established. `environment_id` is `null` for session-cookie
(dashboard) connections, which aren't scoped to a single environment. SDK clients should clear
their local store on receiving this event to prepare for a clean bootstrap, and use
`environment_id` to route impression reporting (`POST /api/environments/{environment_id}/impressions`).

### Event: `update`

```
event: update
data: {"type":"UPSERT","env_id":"<uuid>","flag":{"key":"my-flag","is_enabled":true,...}}
```

```
event: update
data: {"type":"DELETE","env_id":"<uuid>","key":"my-flag"}
```

### Event: `ready`

```
event: ready
data: "3"
```

Sent once the full bootstrap replay (the `update` events above) is complete — `data` is the
number of flags sent. SDK clients should resolve their `connect()` promise on this event rather
than guessing when bootstrap has finished. Sent on every (re)connection, not just the first.

### Keep-Alive

```
: keep-alive-text
```

Sent every 15 seconds to prevent proxy timeouts.

### Reconnection

If the SSE connection drops, clients should reconnect with exponential backoff. On reconnect:

1. Server sends `connected` → SDK clears local store
2. Server replays all current flags for the environment → SDK rebuilds from scratch
3. Server sends `ready` → SDK resolves `connect()` again if it hadn't already

This guarantees consistency even after missed updates during the disconnected period.

---

## Flags Snapshot (Poll Fallback)

```http
GET /flags/snapshot
Authorization: Bearer sk_live_your_key
```

Returns the full, segment-expanded flag set for the environment associated with the SDK key, as
a plain JSON array — the same shape each flag has in an SSE `update` event, but without the
`type`/`env_id` envelope:

```json
[
  { "key": "my-flag", "is_enabled": true, "rollout_percentage": null, "rules": [], "flag_type": "boolean", ... }
]
```

The response header `X-Checkgate-Environment-Id` identifies the environment for impression and conversion reporting. The response body remains a plain array, and the header is exposed through CORS.

Intended as a fallback for SDK clients when the SSE connection at `/stream` cannot be
established at all — e.g. a corporate proxy or firewall blocking long-lived connections. Official
SDKs poll this endpoint automatically after a configurable number of consecutive failed SSE
reconnects (default: 3), and stop polling as soon as SSE reconnects successfully.

Unlike `GET /api/environments/{env_id}/flags` (user or SDK-key auth, segment references left
unexpanded — intended for the dashboard UI), this route is keyed by SDK key alone, exactly like
`/stream`, and returns segment-expanded flags ready to evaluate against directly.

Returns `400` if called with session-cookie auth rather than an SDK key (the dashboard isn't
scoped to a single environment the way an SDK key is), and `401` for missing/invalid credentials.

---

## Health Check

```http
GET /health
```

Public endpoint — no authentication required.

**Response** `200 OK` — body: `OK`

Use this for load balancer and container health checks.
