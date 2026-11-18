# Observability review: v0.1.31

v0.1.30 introduced the metrics, probes and structured logs that this review
audits. The canonical public reference is
[`docs/operations/observability.md`](../../operations/observability.md).

## Coverage against the milestone's required areas

| Area | Signal | Status |
|---|---|---|
| **Evaluation:** counts, duration, success/failure, fallback usage | Evaluation happens inside the SDK process and never reaches the server, so server metrics cannot count it. Impressions (opt-in, batched) are the existing per-evaluation signal; the SDKs log a warning when called before ready. | **NOT APPLICABLE server-side.** SDK-side metrics hooks would be a feature (left out of scope). |
| **Configuration freshness** | `checkgate_sync_divergences_total` (lost notifications), `checkgate_resync_requests_total{trigger}`, `checkgate_sync_tracked_flags`, `checkgate_redis_connected` | VERIFIED. One new trigger value, `segment_read_failed` (R2). |
| **Server:** throughput, latency, errors | `checkgate_http_requests_total{method,route,status_class}`, `checkgate_http_request_duration_seconds{method,route}` | VERIFIED |
| Active connections | `checkgate_sse_active_connections`, `_connections_total`, `_rejections_total{reason}`, `_disconnects_total{reason}` | VERIFIED |
| **CPU and memory** | none | **Gap → added**: `process_cpu_seconds_total`, `process_resident_memory_bytes`, `process_virtual_memory_bytes`, `process_open_fds`, `process_threads`, `process_start_time_seconds` (Linux) |
| Queue / backlog | `checkgate_sse_lagged_messages_total`; bootstrap wait is included in `checkgate_sse_bootstrap_duration_seconds`; `checkgate_resync_pending`; DB pool gauges | VERIFIED |
| **Propagation:** update processing latency | `checkgate_flag_publish_duration_seconds` (publish side), `checkgate_flag_updates_received_total` | PARTIALLY VERIFIED. End-to-end propagation (commit → SDK) is measured by the public benchmark (`bench/public/sdk/propagation.mjs`), not by a server metric: the server never sees the SDK apply the change. |
| Delayed updates, reconnection and recovery | divergences, resyncs by trigger, `checkgate_redis_reconnections_total{reason}`, SSE disconnect reasons | VERIFIED |
| **Diagnostics:** structured logs | JSON or text via `LOG_FORMAT`, `RUST_LOG` filter; the event catalogue is in observability.md | VERIFIED. Two leaks fixed: `?sdk_key=` in the request span (S7) and the integration URL in delivery errors (S4) |
| Readiness and liveness | `/health/live` (never fails on a dependency), `/health/ready` (drain, DB `SELECT 1` within 2 s, optional Redis) | VERIFIED (`tests/lifecycle.rs`) |
| Metrics endpoint | `/metrics` on a separate listener, off unless `METRICS_ADDR` is set | VERIFIED |

## Cardinality

Every label is from a fixed set: route **templates** (`/api/environments/{env_id}/flags`,
never concrete IDs), method, status class, and a closed set of reasons,
triggers and outcomes. No label carries a user key, attribute value, SDK key,
environment id or flag key. The only per-instance series counts that grow are
the fixed route templates (~80). The new process metrics add 6 series with no
labels.

## Overhead

- **Process metrics** cost nothing between scrapes. A scrape reads three small
  `/proc` files and lists `/proc/self/fd`, which is O(open fds): about one per
  SSE connection, so roughly 10 000 directory entries at the default
  connection cap.
- The HTTP middleware (one counter and one histogram per request) and the
  request span existed before; the span now records the path instead of the
  full URI (a little less work).
- With `METRICS_ADDR` unset, nothing is encoded.

## Recommended next steps (not in this release)

1. An SDK-side metrics hook (an `onEvaluation` callback, or a counter) for
   applications that want per-flag evaluation counts without impressions.
2. A per-instance propagation-lag histogram: stamp the commit time into the
   change event and observe it on relay. It needs a wire-format addition, which
   SDKs ignore, so it is backward compatible, but it should be its own change.
