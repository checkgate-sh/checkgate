---
title: Deployment, failure and recovery
description: Health probes, graceful shutdown, capacity limits and troubleshooting procedures for running Checkgate in production.
---

# Deployment, failure and recovery

## Lifecycle

**Startup.** Connect to PostgreSQL and apply migrations, connect to Redis,
subscribe to the change channel, seed the reconciliation index, then listen.
`/health/ready` turns 200 once the server listens and PostgreSQL answers. A
missing or unreachable database stops the process with an error (the
orchestrator restarts it); nothing needs manual repair after an outage.

**Shutdown** (SIGTERM or SIGINT):

1. Readiness fails at once (`503 draining`); new SSE streams are refused with
   `503` + `Retry-After`. Other requests are still served.
2. After `SHUTDOWN_DRAIN_DELAY_SECS` (default 0) — time for load balancers to
   take the instance out of rotation — every SSE stream is closed, the server
   stops accepting connections, and the Redis subscriber, reconciler,
   scheduler, resync retries and limiter cleanup stop.
3. In-flight requests finish, bounded by `SHUTDOWN_GRACE_PERIOD_SECS` (default
   20) counted from the signal; whatever is left is cut off and logged.
4. The PostgreSQL pool is closed and the process exits `0`.

Clients of the stopped instance reconnect through the load balancer to another
instance and re-bootstrap; nothing they held is lost. A change committed by
the stopping instance whose event did not go out is found by the other
instances' reconciliation.

Before this release, an open SSE stream kept the process from ever finishing
a graceful shutdown, so every termination ended in SIGKILL after the
orchestrator's grace period.

## Kubernetes

```yaml
spec:
  terminationGracePeriodSeconds: 30        # > SHUTDOWN_GRACE_PERIOD_SECS
  securityContext:
    runAsNonRoot: true
    runAsUser: 10001
    runAsGroup: 10001
  containers:
    - name: checkgate
      image: ghcr.io/checkgate-sh/checkgate:<version>
      env:
        - { name: CHECKGATE_MODE, value: external }
        - { name: SHUTDOWN_DRAIN_DELAY_SECS, value: "5" }
        - { name: SHUTDOWN_GRACE_PERIOD_SECS, value: "25" }
        - { name: METRICS_ADDR, value: "0.0.0.0:9464" }
        # DATABASE_URL, REDIS_URL, SESSION_SECRET, SETUP_TOKEN from a Secret
      ports:
        - { name: http, containerPort: 3000 }
        - { name: metrics, containerPort: 9464 }
      startupProbe:
        httpGet: { path: /health/live, port: http }
        periodSeconds: 2
        failureThreshold: 60                 # migrations on first start
      livenessProbe:
        httpGet: { path: /health/live, port: http }
        periodSeconds: 10
        failureThreshold: 3
      readinessProbe:
        httpGet: { path: /health/ready, port: http }
        periodSeconds: 5
        failureThreshold: 2
        timeoutSeconds: 3
```

- Keep `SHUTDOWN_DRAIN_DELAY_SECS` ≥ the time your ingress needs to observe a
  failed readiness probe (`periodSeconds × failureThreshold`, here 10 s is
  safe; 5 s works with endpoint-slice–driven ingresses that react to the pod's
  `Terminating` state).
- Keep `terminationGracePeriodSeconds` above `SHUTDOWN_GRACE_PERIOD_SECS` so the
  process exits on its own rather than being killed.
- In external mode the image runs entirely as UID 10001; all-in-one mode needs
  to start as root to own the bundled database's volume, and drops privileges
  for every process.

## Docker

The image's `HEALTHCHECK` probes `/health/live`. For Compose-based rolling
restarts, give the container time to drain:

```yaml
services:
  checkgate:
    stop_grace_period: 30s
    environment:
      SHUTDOWN_GRACE_PERIOD_SECS: "25"
```

External mode runs fully unprivileged with `--user 10001:10001`.

## Load balancers

- **No stickiness needed.** Any instance can serve any client: bootstraps read
  PostgreSQL and every instance relays every change.
- **Idle timeout ≥ 30 s** on the path to `/stream` (keep-alive comments are
  sent every 15 s). AWS ALB's default of 60 s is fine.
- **Do not buffer** `/stream` (`proxy_buffering off;` in nginx, or honour the
  `text/event-stream` content type).
- **Health check** `/health/ready`, interval ≤ 10 s.
- **Deregistration delay** ≈ `SHUTDOWN_DRAIN_DELAY_SECS`, not longer: streams
  are closed by the server after the drain delay anyway.
- Behind the balancer, set `TRUSTED_PROXIES` (and `TRUSTED_PROXY_HEADER` if it
  sends RFC 7239 `Forwarded` instead of `X-Forwarded-For`).

## Capacity limits and overload behaviour

| Setting | Default | When exceeded |
|---|---|---|
| `SSE_MAX_CONNECTIONS` | 10 000 per instance (`0` = unlimited) | `503` + `Retry-After` (2–10 s, jittered). SDKs back off; after 3 failures they poll `/flags/snapshot`. |
| `SSE_BOOTSTRAP_CONCURRENCY` | `DB_MAX_CONNECTIONS / 2` (≥ 1) | Bootstraps and snapshot polls queue for a slot… |
| `SSE_BOOTSTRAP_QUEUE_TIMEOUT_SECS` | 10 | …and get `503` + `Retry-After` after this long. |
| Broadcast capacity | 256 events | A stream that far behind is closed; the SDK reconnects and re-bootstraps. |
| `TCP_USER_TIMEOUT_SECS` | 60 (Linux; `0` disables) | A connection whose sent data stays unacknowledged this long (a client that stopped reading, a vanished network) is aborted by the kernel and its resources freed. |
| `DB_MAX_CONNECTIONS` | 10 | Requests wait up to 10 s for a pool connection, then fail with 500. |
| Rate limits | see [Self-hosting](../self-hosting.md#rate-limits) | `429` + `Retry-After`. |

Sizing guidance (see [baseline](../performance/baseline.md) for measured
figures): an idle SSE stream measured about 27 KB of server memory (RSS) and
one file descriptor, so 10 000 streams need roughly 300 MB per instance. Plan
`SSE_MAX_CONNECTIONS` from your file-descriptor limit (`ulimit -n`) and memory
headroom, and spread fleets of more than a few thousand clients across
instances. Total `DB_MAX_CONNECTIONS × replicas` must stay below PostgreSQL's
`max_connections`.

## Synchronization settings

| Setting | Default | Meaning |
|---|---|---|
| `REDIS_CHANNEL` | `checkgate_updates` | Channel for change events. Give independent deployments that share a Redis different channels. |
| `REDIS_PING_INTERVAL_SECS` | 10 | Subscriber liveness ping; an unanswered ping (timeout ≤ 5 s) reconnects. |
| `SYNC_RECONCILE_INTERVAL_SECS` | 30 (`0` disables) | Reconciliation against PostgreSQL (±20 % jitter). |
| `SYNC_RECONCILE_GRACE_SECS` | 10 | How long a committed change may stay unrelayed before it counts as lost. |
| `SSE_RESYNC_SPREAD_MS` | 5 000 | Resynced streams close at random points within this window. |
| `READY_REQUIRES_REDIS` | `false` | Fail readiness while the Redis subscription is down. |

## Troubleshooting

**Clients see a change late or not at all.**
Check `checkgate_redis_connected` on every instance and
`checkgate_flag_update_publish_failures_total`. If Redis is healthy and
`checkgate_sync_divergences_total` keeps rising, events are being lost between
publish and delivery: look for instances on a different `REDIS_CHANNEL`, Redis
`client-output-buffer-limit pubsub` disconnects (`redis_reconnections_total`),
or a proxy in front of Redis that drops idle connections. Any lost event is
repaired within the reconciliation bound; a change that never arrives on a
connected client is a bug — collect the instance logs.

**Many clients reconnect at once.**
`sse_disconnects_total{reason="resync"}` spikes follow Redis reconnections or
reconciliation repairs. Widen `SSE_RESYNC_SPREAD_MS` if bootstraps then queue
(`sse_rejections_total{reason="bootstrap_busy"}`), or raise
`SSE_BOOTSTRAP_CONCURRENCY` together with `DB_MAX_CONNECTIONS`.

**`lagged` disconnects.** A client (or the instance's CPU) cannot keep up
with the event rate. Bulk changes (a segment edit re-revisions every flag that
uses it) produce bursts; occasional lag disconnects are harmless, a steady rate
means an overloaded instance.

**Readiness flaps.** `checks.database` shows which dependency failed. A
database that answers slower than 2 s under load marks instances not ready;
look at pool saturation (`db_pool_idle_connections` = 0) first.

**Instance takes the whole grace period to stop.** The log line
`Grace period elapsed with requests still in flight` names the open stream
count; long-running REST requests (large audit exports) are the usual cause.
