# Operational diagnostics: v0.1.31

Probes, logs and troubleshooting are documented in
[`docs/operations/observability.md`](../../operations/observability.md) and
[`docs/operations/failure-recovery.md`](../../operations/failure-recovery.md).
This page covers the symptoms that are new or changed in v0.1.31 and how to
diagnose them.

## New error responses

| Symptom | Cause | What to do |
|---|---|---|
| `422` on flag create or patch, log `Rejected create_flag: configuration exceeds limits` / `Rejected patch_flag: …` with `reason` | The flag exceeds a limit: more than 500 rules; a segment referenced twice; more than 50 distinct segments; a prerequisite on itself, listed twice, or more than 50; more than 100 variants. | Read `reason` in the log. Merge the duplicate rules or segment references. These limits are far above observed real-world configurations; if a legitimate one hits them, report it. |
| `422` on segment create or patch, log `Rejected create_segment` / `patch_segment` | More than 500 rules in one segment. | Split the segment, or use one rule with many `values`. |
| `500` from `GET /flags/snapshot`, log `flags_snapshot: failed to read a consistent snapshot` with `flag "…" does not decode` | A stored flag row is not valid for this server version (typically written by a **newer** server during a rolling upgrade, or edited by hand in SQL). Before v0.1.31 the flag was silently left out, and polling SDKs then deleted it locally. | Finish the rollout to one version, or repair the row. Stream clients see the same condition as a failing `/stream` bootstrap (`checkgate_sse_rejections_total{reason="bootstrap_error"}`). |
| `403` on `POST /api/tokens` from a token | The new token would outlive the token creating it (no expiry, or a later expiry). | Request an expiry at or before the caller's own, or create the token from a dashboard session. |
| `403` on `DELETE /api/tokens/{id}` from a read-only token | Revoking needs write scope. | Use a read-write token or a session. |
| Webhook delivery logged with status `3xx` | Redirects are no longer followed (S3). | Point the webhook at the final URL. |
| Webhook or integration delivery `response_body` cut at 1 KiB | Stored response bodies are capped. | Nothing to do; full bodies were never needed for delivery status. |

## New log events

| Message | Level | Fields | Meaning |
|---|---|---|---|
| `Could not read segments to expand a committed flag change — resyncing this environment's clients instead of publishing it` | WARN | `operation`, `env_id`, `error` | The change is committed; local clients re-bootstrap now, and other instances recover through reconciliation within `SYNC_RECONCILE_INTERVAL_SECS` × 1.2 + `SYNC_RECONCILE_GRACE_SECS`. A cluster of these points to database pressure (check the `checkgate_db_pool_*` gauges). |
| `[shutdown] Server exited with status N; stopping Redis and PostgreSQL.` / `[shutdown] Bundled services stopped.` (container stdout, all-in-one mode) | — | — | Clean shutdown of the bundled services (D1). `[warn] PostgreSQL did not stop cleanly` means the stop timeout was too short; raise the orchestrator's grace period. |

## Changed log content

- The request span field is now `path` (the path only), not `uri`. Queries that
  filtered on `uri` must use `path`. Query strings, including `?sdk_key=`, are
  no longer logged.
- Webhook delivery logs no longer include `url`. Correlate by `webhook_id`.
- Delivery errors stored in `webhook_deliveries.error` and
  `integration_deliveries.error` no longer end with ` for url (…)`.

## SDK-side diagnostics

| SDK | Message | Meaning |
|---|---|---|
| React Native | `[Checkgate] No global EventSource. Install an SSE polyfill that supports headers …` | No `EventSource` polyfill is installed. The SDK keeps flags current by polling `/flags/snapshot`. Install `react-native-sse` (see the SDK README). |
| Flutter | (no new message) | Large bootstraps no longer lose flags or the `ready` event (R1). If `connect()` used to hang on large environments, this was the cause. |

## Quick health checklist

```bash
curl -fsS http://HOST:3000/health/live          # process up
curl -fsS http://HOST:3000/health/ready         # draining? database reachable?
curl -s  http://HOST:9090/metrics | grep -E '^(checkgate_(ready|database_up|redis_connected|sse_active_connections)|process_(resident_memory_bytes|open_fds)) '
```
