---
title: "Governance — Approvals, Audit Log, Scheduled Changes & Access Control"
description: "Move fast without breaking things: Checkgate's governance layer — role-based access control, an audit log, change-request approvals, scheduled changes, flag lifecycle metadata, cross-environment diff, and scoped personal access tokens."
---

# Governance

As a flag system grows, "who can change what, when, and how do we review it?" matters as much as
evaluation. Checkgate layers governance on top of flags without slowing down the common case.

## Roles & access control

Three roles gate every action — `admin`, `editor`, and `viewer` — applied both at the workspace
level and per project. See [Core Concepts → Users and Roles](/guide/concepts#users-and-roles) for
the full matrix. In short: editors manage flags; only admins manage users, environments, SDK
keys, and webhooks; viewers are read-only.

## Audit log

Every flag mutation is recorded — **who** changed **what**, **when**, and the **before/after**
state. The **Audit Log** page is a searchable history covering create, update, delete, promote,
archive, and unarchive actions, filterable by flag key. It's the first place to look during an
incident ("what changed right before this broke?"). See the
[API reference](/api-reference#audit-log).

## Change requests (approvals)

Each environment has a **`require_approval`** toggle. When it's on, a flag `PATCH` is **not**
applied immediately — it's captured as a pending **change request** that a *different* editor or
admin must review (self-approval is blocked).

- **Approve** applies the original patch atomically against whatever the flag's current state is
  at approval time — so an approval always lands cleanly on top of the latest data, not a stale
  snapshot. If the update fails, the request remains pending and can be retried.
- **Reject** / **withdraw** never touch the flag.
- It's scoped per environment, so Production can require review while Development stays
  frictionless.

Replacing an existing flag through POST, deleting a flag, promoting into a protected environment, scheduling a change, and editing segments return `409 Conflict` while approval is enabled. Submit a flag PATCH for review instead. Creating a new flag is still allowed.

This gives you a pull-request-style gate on your highest-risk changes. See the
[Change Requests API](/api-reference#change-requests).

A queued request only helps if someone knows it's waiting — wire the environment up to
[Slack or Teams alerts](/ecosystem/chat-alerts) so `change_request.opened` lands in the channel
your reviewers already watch.

::: warning Automation & approvals
If an environment requires approval, programmatic updates (Terraform, the operator, scripts) can't
apply synchronously — the change is queued for review. Point infrastructure-as-code at a
non-gated environment, or approve out of band. The [Terraform provider](/ecosystem/terraform)
surfaces this explicitly rather than hanging.
:::

## Scheduled changes

Queue a flag change to apply at a future time — a launch at 9am, a rollout bump overnight, a
kill-switch flip after a maintenance window. A background worker applies due changes
(multi-instance safe). Manage them on the **Scheduled** page or via the
[Scheduled Changes API](/api-reference#scheduled-changes).

## Flag lifecycle

Keep a growing flag set tidy with dashboard-only metadata that never affects evaluation:

- **Tags** — free-form labels for search and filtering.
- **Owner email** — who's responsible for the flag.
- **Archival** — a reversible soft-delete that hides a flag from the default list with zero
  evaluation impact (archived flags are simply excluded from what SDKs receive).

These live on a metadata wrapper and are never sent to SDKs or over SSE.

## Cross-environment diff

The **Compare environments** view shows where two environments diverge — flags that exist in only
one, or that differ in evaluation-relevant fields (rollout, rules, variants, prerequisites; tags
and ownership are ignored). Each difference offers a one-click sync, complementing the per-flag
**Promote** action for keeping Staging and Production aligned.

## Personal access tokens

For CI/CD, Terraform, and scripts, mint a **personal access token** instead of using an
admin-equivalent SDK key:

- A token acts as its owning user (same role, same project memberships) and can be capped to
  **`read_only`**.
- Tokens are **SHA-256 hashed at rest**, support optional expiry, and are strictly self-service —
  you can only list, create, and revoke your own.
- A `read_only` token cannot mint a more-privileged replacement for itself.

Manage tokens under **Settings → Access Tokens**, or via the
[Personal Access Tokens API](/api-reference#personal-access-tokens).

## See also

- [Core Concepts](/guide/concepts) — flags, environments, and roles.
- [Enterprise Setup](/enterprise-setup) — deploying Checkgate for a whole organization.
- [Terraform provider](/ecosystem/terraform) & [Kubernetes operator](/ecosystem/kubernetes) —
  manage flags as code under the same governance.
