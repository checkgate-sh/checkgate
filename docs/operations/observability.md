---
title: Observability
description: Prometheus metrics, health endpoints and structured logs for operating Checkgate, and how to tell healthy operation from each failure mode.
---

# Observability

## Metrics

Checkgate exposes Prometheus/OpenMetrics metrics at `GET /metrics` on a
**separate listener**, off by default:

```bash
METRICS_ADDR=0.0.0.0:9464   # or 127.0.0.1:9464 for a local agent
```

The metrics port is never served on the public port (`PORT`). Bind it to a
private interface, or keep it unpublished (Docker) / reachable only inside the
cluster (Kubernetes), and scrape it from there. Metrics carry no secrets, but
they do describe traffic and capacity.

```yaml
# Prometheus
scrape_configs:
  - job_name: checkgate
    static_configs: [{ targets: ["checkgate:9464"] }]
```

```yaml
# Kubernetes: container port + PodMonitor/ServiceMonitor, or annotations
ports:
  - { name: http, containerPort: 3000 }
  - { name: metrics, containerPort: 9464 }
env:
  - { name: METRICS_ADDR, value: "0.0.0.0:9464" }
```

Labels are fixed, low-cardinality sets: route *templates*
(`/api/environments/{env_id}/flags/{key}`, never a raw path), status classes
(`2xx`…`5xx`) and small enums. Flag keys, environment ids, SDK keys, users,
addresses and connection ids never appear in metrics. Every enumerated series
exists from start-up at `0`, so `rate()` and alerts work before the first
event. Recording is lock-free on the request path.

The instrumentation uses `prometheus-client`, whose data model is OpenMetrics;
an OpenTelemetry collector can scrape the same endpoint (Prometheus receiver)
to forward metrics to any OTel backend.

### Reference

| Metric | Type | Meaning |
|---|---|---|
| `checkgate_http_requests_total{method,route,status_class}` | counter | Requests handled. Unmatched paths (dashboard files) use `route="static"`. |
| `checkgate_http_request_duration_seconds{method,route}` | histogram | Time to the response head. For `/stream` that is the bootstrap, not the stream's lifetime. |
| `checkgate_flag_updates_total` | counter | Change events this instance published (or tried to). |
| `checkgate_flag_update_publish_failures_total` | counter | Events Redis did not accept after a retry. Delivered locally; other instances recover by resync. |
| `checkgate_flag_updates_received_total` | counter | Events received from the Redis subscription (from every instance, including this one). |
| `checkgate_flag_publish_duration_seconds` | histogram | Time to publish one event, retry included. |
| `checkgate_redis_connected` | gauge | `1` while the subscription is up and answering pings. |
| `checkgate_redis_reconnections_total{reason}` | counter | Subscription losses: `closed`, `ping_timeout`, `connect_failed`. |
| `checkgate_resync_requests_total{trigger}` | counter | Times clients were told to re-bootstrap: `subscriber_reconnect`, `remote`, `reconciliation`, `environment_removed`. |
| `checkgate_resync_failures_total` | counter | Failed attempts to broadcast a `RESYNC` through Redis. |
| `checkgate_resync_pending` | gauge | `1` while a `RESYNC` broadcast is waiting for Redis. |
| `checkgate_sync_reconciliations_total{outcome}` | counter | Reconciliation passes, `success`/`failure`. |
| `checkgate_sync_reconcile_duration_seconds` | histogram | Duration of one pass. |
| `checkgate_sync_divergences_total` | counter | Environments found with committed changes never relayed — **lost notifications**. |
| `checkgate_sync_tracked_flags` | gauge | Entries in the reconciliation index. |
| `checkgate_sse_connections_total` | counter | Streams opened. |
| `checkgate_sse_rejections_total{reason}` | counter | Streams refused: `capacity`, `bootstrap_busy`, `bootstrap_error`, `draining`. |
| `checkgate_sse_disconnects_total{reason}` | counter | Streams ended: `client`, `lagged`, `resync`, `shutdown`. |
| `checkgate_sse_lagged_messages_total` | counter | Events skipped by clients that fell behind (each such client is disconnected). |
| `checkgate_sse_active_connections` | gauge | Open streams on this instance. |
| `checkgate_sse_bootstrap_duration_seconds` | histogram | Bootstrap read time, including waiting for a slot. |
| `checkgate_snapshot_query_duration_seconds` | histogram | `/flags/snapshot` read time, including waiting for a slot. |
| `checkgate_db_pool_connections` / `_idle_connections` / `_max_connections` | gauge | PostgreSQL pool. |
| `checkgate_database_up` | gauge | `1` if the last database check (readiness or reconciliation) succeeded. |
| `checkgate_ready` | gauge | `1` while ready (not draining). |
| `checkgate_build_info{version}` | info | Build version. |

### Reading the signals

| Situation | What you see |
|---|---|
| Healthy | `redis_connected=1`, `database_up=1`, `ready=1`; `sync_divergences_total` flat; `flag_update_publish_failures_total` flat; `sse_disconnects_total{reason="lagged"}` near 0. |
| Redis unavailable | `redis_connected=0`; `redis_reconnections_total{reason="connect_failed"}` rising; `flag_update_publish_failures_total` rising on writes; `resync_pending=1`; `/health/ready` reports `degraded` (still 200 unless `READY_REQUIRES_REDIS=true`). |
| Redis connection silently dead | `redis_reconnections_total{reason="ping_timeout"}` increments, then `resync_requests_total{trigger="subscriber_reconnect"}`. |
| Database unavailable | `database_up=0`; `sync_reconciliations_total{outcome="failure"}` rising; `5xx` on API routes; `sse_rejections_total{reason="bootstrap_error"}`; `/health/ready` 503. |
| Growing SSE load | `sse_active_connections` rising toward `SSE_MAX_CONNECTIONS`; then `sse_rejections_total{reason="capacity"}`. |
| Reconnect wave / slow database | `sse_bootstrap_duration_seconds` p95 rising; `sse_rejections_total{reason="bootstrap_busy"}`; `db_pool_idle_connections` at 0. |
| Lagging clients | `sse_disconnects_total{reason="lagged"}` and `sse_lagged_messages_total` rising. |
| Lost notifications repaired | `sync_divergences_total` and `resync_requests_total{trigger="reconciliation"}` increment, with a `WARN` log naming the environment. |
| Persistent publish errors | `flag_update_publish_failures_total` rising while `redis_connected=1`: the publisher connection (not the subscriber) is failing — check Redis `maxclients`, ACLs, network policy. |

### Suggested alerts

```yaml
groups:
  - name: checkgate
    rules:
      - alert: CheckgateRedisDisconnected
        expr: max_over_time(checkgate_redis_connected[5m]) == 0
        for: 5m
      - alert: CheckgateDatabaseDown
        expr: checkgate_database_up == 0
        for: 2m
      - alert: CheckgateLostNotifications
        # Occasional repairs are the system working; a steady rate means events
        # are being lost (publisher crashes, Redis trouble).
        expr: increase(checkgate_sync_divergences_total[1h]) > 3
      - alert: CheckgatePublishFailures
        expr: increase(checkgate_flag_update_publish_failures_total[10m]) > 0
      - alert: CheckgateSseNearCapacity
        expr: checkgate_sse_active_connections > 0.8 * 10000   # your SSE_MAX_CONNECTIONS
      - alert: CheckgateSlowBootstrap
        expr: histogram_quantile(0.95, rate(checkgate_sse_bootstrap_duration_seconds_bucket[5m])) > 1
        for: 10m
```

## Health endpoints

| Endpoint | Purpose | Fails when |
|---|---|---|
| `GET /health/live` | Liveness | Never while the process serves requests. A dependency outage never fails it, so it cannot cause restart loops. |
| `GET /health/ready` | Readiness — route traffic here? | Draining for shutdown; PostgreSQL does not answer `SELECT 1` within 2 s; Redis subscription down **only if** `READY_REQUIRES_REDIS=true`. |
| `GET /health` | Legacy, unchanged (`OK`) | Same as liveness. |

`/health/ready` answers JSON:

```json
{ "status": "ready", "checks": { "database": "ok", "redis": "ok" } }
```

`status` is `ready`, `degraded` (Redis down, still serving: writes commit and
reach this instance's clients; other instances recover by reconciliation), 
`unavailable` (503) or `draining` (503). None of the health endpoints is rate
limited.

Why Redis does not fail readiness by default: PostgreSQL is authoritative, and
every read path (bootstrap, snapshot, REST) works without Redis. Taking every
instance out of rotation during a Redis outage would turn a propagation delay
into a full outage. Set `READY_REQUIRES_REDIS=true` if you prefer to drain an
instance whose own path to Redis is broken while others remain healthy.

## Logs

Logs are JSON lines on stdout (`RUST_LOG` sets the level). Stable messages
and fields for operations:

| Event | Level | Fields |
|---|---|---|
| `Redis subscriber: listening` | INFO | `channel` |
| `Redis subscriber: connection lost — reconnecting` | WARN | `reason` |
| `Redis subscriber: ping unanswered` / `ping failed` | WARN | `timeout` / `error` |
| `Redis subscriber: resubscribed — resyncing local SSE clients` | WARN | |
| `Redis publish failed — delivered locally, other instances will resync` | WARN | `operation`, `error` |
| `Redis reachable again — resync broadcast sent` | INFO | |
| `Committed flag changes were never relayed to this instance — resyncing the environment's clients` | WARN | `environment_id`, `flags`, `sample_keys` |
| `Sync reconciliation failed — will retry` | WARN | `error` |
| `SSE client connected` | INFO | `connection_id`, `environment_id`, `sdk_key_name`, `bootstrap_flags`, `bootstrap_ms`, `client_ip` |
| `SSE client disconnected` | INFO | `connection_id`, `reason` |
| `SSE client lagged — closing connection to trigger reconnect` | WARN | `missed_messages` |
| `SSE connection refused — instance at SSE_MAX_CONNECTIONS` | WARN | |
| `Shutdown requested` / `Draining: …` / `Server shut down gracefully` | INFO | `signal`, `drain_delay`, `grace_period`, `open_streams`, `elapsed_ms` |
| `Scheduler: …` | INFO/ERROR | `id`, `key`, `error` |

SDK keys, session cookies, personal access tokens, setup tokens, user
attributes and connection-string passwords are never logged (connection
strings are not logged at all; the image's entrypoint logs them redacted).
