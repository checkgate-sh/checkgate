---
title: "Slack & Microsoft Teams Alerts"
description: "Deliver flag changes and change-request activity into Slack or Microsoft Teams. Per-environment configuration, per-event filtering, native Block Kit and MessageCard formatting, and a bounded delivery log."
---

# Slack & Teams Alerts

Flag changes matter to people who never open the dashboard. Chat alerts push flag and
change-request activity into the channel a team already watches — a native **Slack Block Kit**
message or a **Teams MessageCard**, not a raw JSON blob.

Alerts are configured **per environment**, so Production can be noisy in `#incidents` while
Development stays quiet.

::: tip Alerts, not approvals
This is one-way: Checkgate posts to your channel. Approving a change request from the message
itself is [on the roadmap](/roadmap) — it needs a publicly reachable signed callback endpoint and a
Slack↔Checkgate identity mapping, so that the "you can't approve your own request" rule stays
enforceable. For now, review happens in the dashboard; the alert tells you it's waiting.
:::

## Setup

### 1. Create an incoming webhook

::: code-group

```text [Slack]
Slack → Apps → search "Incoming Webhooks" → Add to Workspace
→ pick a channel → copy the Webhook URL

https://hooks.slack.com/services/<workspace-id>/<channel-id>/<secret-token>
```

```text [Microsoft Teams]
Teams → the channel → ⋯ → Connectors → Incoming Webhook → Configure
→ name it, optionally upload an icon → Create → copy the URL

https://outlook.office.com/webhook/...
```

:::

### 2. Connect it in Checkgate

In the dashboard, open **Chat Alerts** (under the Environment section of the sidebar), pick the
provider, paste the URL, and choose which events to receive. Admin role required.

Then hit **Send test message** on the row — it fires a sample event through the *real* delivery
path, so a message landing in the channel confirms the whole chain, not just that the URL parses.

## Events

| Event | Fires when |
| --- | --- |
| `flag.created` | A flag is created or replaced |
| `flag.updated` | A flag is patched (directly, on schedule, or via an approved change request) |
| `flag.deleted` | A flag is deleted |
| `flag.promoted` | A flag's config is copied into another environment |
| `change_request.opened` | A patch is queued for approval and is waiting on a reviewer |
| `change_request.approved` | A reviewer approved a queued patch — records **who** reviewed it |
| `change_request.rejected` | A reviewer rejected it, with the stated reason |

**Leaving the event selection empty subscribes to everything.** That's the useful default; narrow it
when a channel only cares about one slice — say `change_request.opened` in `#leads`, so reviewers get
pinged without also seeing every Development toggle.

Messages carry the environment name, flag key, enabled/disabled state, rollout percentage, the acting
user, and — for rejections — the reason. Change-request events are colour-coded so a queued request
(amber) is visually distinct from an approval (green) or a rejection/deletion (red).

## Security

The incoming-webhook URL is a **bearer credential** — anyone holding it can post into your channel.
Checkgate treats it that way:

- **It is never returned by the API after creation.** Listing integrations returns only an elided
  preview (`…XXXXXX`), enough to tell two rows apart and useless to an attacker. To change it, write
  a new URL; you cannot read the old one back.
- **Plaintext `http://` is rejected** outside loopback, so the credential is never sent over an
  unencrypted connection. `http://127.0.0.1`, `http://localhost`, and `http://[::1]` are permitted
  for local relays; lookalike hosts such as `http://127.0.0.1.evil.com` are not.
- **Managing integrations is admin-only**, the same tier as webhooks.

If a URL leaks, revoke it at the provider (Slack: delete the webhook in the app config; Teams: remove
the connector) — that invalidates it immediately, whether or not the Checkgate row is deleted.

## Delivery & troubleshooting

Delivery is **fire-and-forget** — it never blocks the API response that triggered it. A POST that
fails to complete (connection or timeout error) is tried up to 3 times, 1 s and then 5 s apart; an
HTTP error response is recorded and not retried. Redirects are not followed. Every attempt is
recorded:

```bash
curl -H "Authorization: Bearer $CHECKGATE_TOKEN" \
  "$CHECKGATE_URL/api/environments/$ENV_ID/integrations/$ID/deliveries"
```

Each row carries the event, HTTP status, response body, and any transport error. The log retains the
most recent 200 deliveries per integration.

**Nothing arriving?** Work down this list:

1. **Is the integration enabled?** The toggle on the row gates delivery entirely.
2. **Does the event filter include what you expect?** A filter set to `flag.deleted` stays silent for
   every other event. Empty means all.
3. **Are you watching the right environment?** Integrations are per environment — a flag change in
   Development will not notify a channel wired to Production.
4. **Check the delivery log.** A `4xx` means the provider rejected the message (usually a revoked or
   mistyped URL); a transport error means we could not reach the host at all.

## API

Full reference: [REST API → Integrations](/api-reference#integrations-slack-microsoft-teams). All routes are environment-scoped:

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/environments/{env}/integrations` | List (URLs elided) |
| `POST` | `/api/environments/{env}/integrations` | Create |
| `PATCH` | `/api/environments/{env}/integrations/{id}` | Update name, URL, events, enabled |
| `DELETE` | `/api/environments/{env}/integrations/{id}` | Disconnect |
| `POST` | `/api/environments/{env}/integrations/{id}/test` | Send a sample message |
| `GET` | `/api/environments/{env}/integrations/{id}/deliveries` | Recent delivery attempts |

```bash
curl -X POST "$CHECKGATE_URL/api/environments/$ENV_ID/integrations" \
  -H "Authorization: Bearer $CHECKGATE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "kind": "slack",
    "name": "#engineering",
    "webhook_url": "https://hooks.slack.com/services/<workspace-id>/<channel-id>/<secret-token>",
    "events": ["change_request.opened", "flag.deleted"]
  }'
```

`kind` is `slack` or `teams`. Unknown providers, non-loopback plaintext URLs, and unrecognised event
names are all rejected with `422`.

## Raw webhooks vs chat alerts

Both live under the Environment section, and they are not the same tool:

- **[Webhooks](/api-reference#webhooks)** post Checkgate's own JSON envelope to any endpoint, signed
  with HMAC-SHA256 so the receiver can verify it. Use them to *drive automation* — trigger a
  pipeline, sync a system, feed an incident tool.
- **Chat alerts** post a provider-shaped message to Slack or Teams, with no signature, because the
  URL itself is the credential. Use them to *inform humans*.

Both receive the same events, so a channel and a pipeline can react to the same change.
