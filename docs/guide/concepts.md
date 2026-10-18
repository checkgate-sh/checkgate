---
title: "Core Concepts — Projects, Environments, Flags, Targeting & Rollouts"
description: "Understand Checkgate's building blocks: the workspace/project/environment hierarchy, flag schema, targeting rule operators, deterministic rollout hashing, user context, impression tracking, and the SSE update channel."
---

# Core Concepts

## Hierarchy

Checkgate organizes everything into a four-level hierarchy:

```
Workspace (singleton — one per installation)
  └── Projects  (e.g. "Mobile App", "Web App", "API Service")
        ├── Members  (per-project user roles)
        └── Environments  (e.g. Production, Staging, Development)
              ├── SDK Keys  (per-environment, auth for SDK clients)
              ├── Flags     (per-environment configuration)
              └── Impressions
```

Each **project** is a fully isolated space: its own environments, flags, SDK keys, and team members. Changes in one project never affect another.

## Projects

A project represents a single application or service. You might have separate projects for your mobile app, your backend API, and an internal tooling suite — each with independent flag configurations and team membership.

### Creating Projects

The first project is created during the **setup wizard**. Additional projects can be added from the **Projects** page (admin only). Each new project is automatically seeded with three environments: Production, Staging, and Development.

### Project Membership

Users can be members of one or more projects with a per-project role. A **workspace admin** always has full access to all projects regardless of membership.

## Environments

Environments are isolated flag namespaces within a project. Every flag belongs to exactly one environment, so Production, Staging, and Development each have their own independent configuration.

Three environments are seeded automatically when a project is created:

| Environment | Color | Notes |
|-------------|-------|-------|
| Production | Red | |
| Staging | Amber | |
| Development | Green | Default — used when no environment is specified |

The active environment is shown in the dashboard sidebar and can be switched at any time. The **Promote** action copies a flag's configuration from one environment to another in a single atomic transaction.

## Flags

A feature flag is the central object in Checkgate. Every flag has:

| Field | Type | Description |
|-------|------|-------------|
| `key` | `string` | Unique identifier within an environment (e.g. `new-checkout-flow`) |
| `is_enabled` | `boolean` | Master on/off switch |
| `rollout_percentage` | `number \| null` | 0–100 percentage rollout, or null for 100% |
| `description` | `string \| null` | Human-readable description |
| `rules` | `TargetingRule[]` | Optional targeting rules |

### Flag Key Naming

Use lowercase kebab-case or snake_case for flag keys: `new-checkout-flow`, `beta_dashboard`, `dark-mode-v2`. Keys must contain only ASCII alphanumerics, underscores, or hyphens, and are at most 100 characters. Keys are immutable after creation — to rename a flag, delete and recreate it.

Flag keys are unique **per environment**. The same key can exist in multiple environments with different configurations.

## Targeting Rules

Targeting rules let you enable a flag for specific users regardless of the rollout percentage.

A rule has three parts:

| Field | Type | Description |
|-------|------|-------------|
| `attribute` | `string` | User attribute to match (e.g. `email`, `plan`) |
| `operator` | `Operator` | How to compare the attribute |
| `values` | `string[]` | List of values to match against |

### Operators

| Operator | Description | Example |
|----------|-------------|---------|
| `equals` | Exact match (any value in the list) | `plan` equals `["enterprise"]` |
| `not_equals` | No match in the list | `plan` not_equals `["free"]` |
| `contains` | Substring match | `email` contains `["@acme.com"]` |
| `starts_with` | Prefix match | `region` starts_with `["eu-"]` |
| `ends_with` | Suffix match | `email` ends_with `["@contractor.com"]` |
| `greater_than` | Numeric `>` (any value) | `age` greater_than `["18"]` |
| `greater_than_or_equal` | Numeric `≥` | `age` greater_than_or_equal `["18"]` |
| `less_than` | Numeric `<` | `cart_total` less_than `["100"]` |
| `less_than_or_equal` | Numeric `≤` | `score` less_than_or_equal `["0.5"]` |

The numeric operators parse both the attribute and the rule values as numbers (integers or
floats). A non-numeric attribute value never matches, so a `greater_than` rule will not admit
users whose attribute is missing or non-numeric.

### Rule Evaluation

- Rules are evaluated in order; **first match wins**
- If a rule matches, the flag returns `true` immediately (bypasses rollout %)
- If no rule matches, evaluation falls through to the rollout percentage
- A user missing the targeted attribute satisfies `not_equals` (they are "not X"), but fails all other operators

### Example: Internal Beta

Enable a flag for all employees regardless of the 5% global rollout:

```json
{
  "key": "ai-assistant",
  "is_enabled": true,
  "rollout_percentage": 5,
  "rules": [
    {
      "attribute": "email",
      "operator": "ends_with",
      "values": ["@yourcompany.com"]
    }
  ]
}
```

Result:
- `alice@yourcompany.com` → `true` (rule match)
- `bob@customer.com` → 5% chance of `true` (rollout hash)

## Segments

A **segment** is a reusable, named set of targeting rules — define an audience like
"Internal Employees" or "Beta Users" once, then reference it from any flag instead of
copy-pasting the same rules everywhere.

```json
// Segment "internal-employees"
{
  "key": "internal-employees",
  "name": "Internal employees",
  "rules": [
    { "attribute": "email", "operator": "ends_with", "values": ["@yourcompany.com"] }
  ]
}
```

```json
// A flag referencing the segment by key
{
  "key": "ai-assistant",
  "is_enabled": true,
  "rollout_percentage": 5,
  "rules": [
    { "segment_key": "internal-employees" }
  ]
}
```

- Segments are **expanded server-side** into their concrete rules before flags are broadcast,
  so SDKs stay simple — they never need to know segments exist.
- Editing a segment automatically re-broadcasts every flag that references it, so audiences
  update everywhere at once.
- A rule that references a `segment_key` can also carry a `variant`, which propagates to the
  segment's rules that don't specify their own.

See the [Segments guide](/guide/segments) for full details, and the
[API reference](/api-reference#segments) for the CRUD endpoints.

## Rollout Percentage

The rollout percentage enables gradual feature releases. Checkgate uses **deterministic consistent hashing** (MurmurHash3) to assign users to buckets:

- The same user always gets the same result for the same flag
- Buckets are stable across restarts and server instances
- Increasing from 10% to 20% enables the flag for a new cohort without disrupting the existing 10%

### Special Values

- `null` (no rollout) — effectively 100%; all users get `true` if enabled
- `0` — no one gets `true` (useful to disable without deleting)
- `100` — everyone gets `true`

## Weighted Variants (A/B Testing)

For string, integer, and JSON flags, `variants` distributes traffic across **multiple**
values by weight — e.g. a 60/30/10 split across `"control"` / `"treatment-a"` /
`"treatment-b"` — instead of always returning a single `default_value`. This is the
building block for A/B/n experiments.

```json
{
  "key": "checkout-experiment",
  "is_enabled": true,
  "flag_type": "string",
  "variants": [
    { "weight": 60, "value": "control" },
    { "weight": 30, "value": "treatment-a" },
    { "weight": 10, "value": "treatment-b" }
  ]
}
```

- Applies only when the flag is enabled, the user is inside `rollout_percentage`, and no
  targeting rule matched — rules and rollout still take priority, exactly as they do for
  `default_value`.
- Weights don't need to sum to 100 — they're normalized against their total (`[6, 3, 1]`
  behaves identically to `[60, 30, 10]`).
- Bucketing is deterministic and sticky per user (same MurmurHash3 approach as rollout
  percentage, salted independently so variant assignment doesn't correlate with which users
  pass the rollout gate).
- Leave `variants` empty (the default) to keep returning `default_value` as before — fully
  backward compatible.

## Prerequisite Flags

A flag can require another flag to be enabled — or resolved to a specific value — before its
own rules and rollout are even considered. This is checked **first**, ahead of targeting rules
and the rollout percentage: if any prerequisite is unsatisfied, the flag evaluates as disabled
regardless of its own `is_enabled`/rules/rollout configuration.

```json
{
  "key": "advanced-search",
  "is_enabled": true,
  "prerequisites": [
    { "flag_key": "search-v2-infra" },
    { "flag_key": "theme", "required_value": "dark" }
  ]
}
```

- `flag_key` — the prerequisite flag's key.
- `required_value` (optional) — the specific value the prerequisite must resolve to. Omit it to
  just require the prerequisite be enabled (the common case for boolean prerequisites, which
  have no other meaningful value to check).
- Prerequisites are evaluated recursively — a prerequisite can itself have prerequisites — with
  a depth limit that fails closed on cycles (e.g. flag A requiring flag B which requires flag A
  back) or excessively deep chains, rather than looping forever.
- A prerequisite referencing a flag that no longer exists is treated as unsatisfied (fails
  closed) rather than silently passing.
- Leave `prerequisites` empty (the default) for a flag that evaluates independently — fully
  backward compatible.

## User Context

When evaluating a flag, you provide a user context:

```typescript
client.isEnabled('flag-key', 'user-id', {
  email: 'alice@example.com',
  plan: 'pro',
  region: 'eu-west-1',
})
```

- `user-id` — the stable identifier used for rollout hashing (typically your database user ID or UUID)
- Attributes — arbitrary key-value pairs used for targeting rule matching

Attributes are **never sent to the server** — they are only used locally for rule evaluation.

## Impression Tracking

SDKs can report flag evaluation events back to the server asynchronously. This powers the **Impressions** dashboard page.

An impression payload:

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

- Sent as a batch `POST` to `/api/environments/{env_id}/impressions`
- Authenticated with an SDK Bearer key
- Fire-and-forget — evaluation is never blocked waiting for the report
- Up to 500 impressions per batch

### Impressions Dashboard

The Impressions page has two tabs:

**Analytics** — aggregate statistics per flag: total evaluations, true/false split, unique user count, last seen timestamp.

**Stream** — a live evaluation log that auto-refreshes every 3 seconds. Useful for debugging "why isn't this flag working for that user?". Filters by flag key, user ID, and evaluated value. Click any row to expand the full evaluation context JSON.

Impressions also feed two higher-level analytics views:

- **[Exposure dashboards](/guide/experimentation#exposure-dashboards)** — for a given flag, which users are being exposed to which variant, with a per-variant distribution and a daily timeline.
- **[Experiments (A/B testing)](/guide/experimentation#a-b-testing)** — pair a flag with a conversion goal event and Checkgate computes each variant's conversion rate, uplift, and statistical significance.

## Governance

For larger teams, Checkgate adds a governance layer on top of flags — an **audit log** of every
change, per-environment **change-request approvals**, **scheduled changes**, flag **lifecycle**
metadata (tags, owner, archival), a **cross-environment diff**, and revocable **personal access
tokens** for CI/CD. See the [Governance guide](/guide/governance) for the full picture.

## Users and Roles

Checkgate has three roles:

| Role | Access |
|------|--------|
| `admin` | Full access: create/edit/delete flags, manage environments, users, projects, and SDK keys |
| `editor` | Can create and edit flags; cannot manage users, projects, or SDK keys |
| `viewer` | Read-only access to flags and impressions |

### Workspace Admin vs Project Member

- A **workspace admin** has full access to all projects, environments, and users.
- Other users are granted access per-project via the **Members** tab in Project Settings. Each project membership has its own role (`admin`, `editor`, or `viewer`) that is independent of any other project.
- Removing a user from a project revokes their access to that project's flags and environments; it does not delete their account.

Users authenticate with **email and password** through the dashboard login page. The first admin account is created during the setup wizard. Additional users are managed from the **Users** page (workspace admin only).

User sessions are stored in an HttpOnly, AES-256-GCM encrypted cookie with a 7-day TTL.

## SDK Keys

SDK keys (`sk_live_...`) authenticate SDK clients. Each key is tied to a specific **environment** — so the key implicitly identifies both the project and the environment that SDK clients will receive flags from. SDK keys ship inside apps, so they only reach the SDK endpoints (streaming and reading that environment's flags, reporting its impressions and events); programmatic API access uses a personal access token.

- Keys are managed from the **Project Settings → SDK Keys** tab
- Multiple keys per project are supported (e.g. one per service or platform)
- One key is auto-generated for the Production environment on first boot
- Keys are shown in full only once — copy them immediately after creation
- Revoking a key invalidates it instantly; SDK clients using it will receive 401 errors
- The legacy `SDK_KEY` environment variable is also accepted for backwards compatibility

## SSE Stream

The SSE stream (`GET /stream`) is the connection between the server and each SDK client. It uses the W3C Server-Sent Events standard.

### Events

| Event | Payload | Description |
|-------|---------|-------------|
| `connected` | `"true"` | Connection established; SDK resets its store |
| `update` | JSON string | A flag was upserted or deleted |
| (keep-alive) | `keep-alive-text` | Heartbeat every 15 seconds |

### Update Payload

```json
// Upsert
{"type": "UPSERT", "env_id": "<uuid>", "flag": { ...flag object... }}

// Delete
{"type": "DELETE", "env_id": "<uuid>", "key": "flag-key"}
```

The stream is **scoped to the environment identified by the SDK key**. When a client connects, the server looks up the key's `environment_id`, bootstraps only the flags for that environment, and filters all live updates to that environment. Clients only ever see flags for their own project and environment.

## Authentication

Checkgate uses two separate authentication mechanisms:

### Dashboard Users (Session Cookie)

Human users log in with **email and password** at `/login`. On success, the server issues an HttpOnly encrypted session cookie (`lg_session`). The cookie is:

- `HttpOnly` — not accessible to JavaScript
- `SameSite=Strict` — not sent on cross-origin requests (primary CSRF protection)
- AES-256-GCM encrypted — tamper-proof
- Valid for 7 days

### SDK Clients (Bearer Token)

SDK clients and CI/CD automation authenticate with an SDK key:

```http
Authorization: Bearer sk_live_your_key_here
```

For browser `EventSource` connections that cannot set custom headers, the key can also be passed as a query parameter:

```
GET /stream?sdk_key=sk_live_your_key_here
```

All API and stream endpoints require authentication. There is no "open mode."
