# Metrics reference: v0.1.31

The full, maintained reference (every metric, how to read it, suggested
alerts) is [`docs/operations/observability.md`](../../operations/observability.md#metrics).
This page lists only what changed in v0.1.31.

Enable metrics by setting `METRICS_ADDR` (for example `0.0.0.0:9090`); they are
served at `GET /metrics` on that separate listener, in OpenMetrics text format.

## Added

| Metric | Type | Labels | Notes |
|---|---|---|---|
| `process_cpu_seconds_total` | counter | — | User + system CPU time. `rate()` gives cores in use. |
| `process_resident_memory_bytes` | gauge | — | RSS from `VmRSS`. |
| `process_virtual_memory_bytes` | gauge | — | `vsize`. |
| `process_open_fds` | gauge | — | Roughly 1 per SSE stream plus the DB and Redis pools. Alert well below `ulimit -n`. |
| `process_threads` | gauge | — | Tokio workers plus the blocking pool. |
| `process_start_time_seconds` | gauge | — | Restart detection: `changes(process_start_time_seconds[1h]) > 0`. |

These are Linux only (read from `/proc/self` at scrape time) and absent on
other systems.

## Changed

| Metric | Change |
|---|---|
| `checkgate_resync_requests_total{trigger}` | New label value `segment_read_failed`: a flag change committed, its environment's segments could not be read, so this instance's clients of that environment re-bootstrap from PostgreSQL instead of receiving a change with its segment rules stripped (R2). Other instances recover through reconciliation (`checkgate_sync_divergences_total`). |

## Suggested additions to existing alerts

```promql
# Memory growth on a steady fleet (a leak, or SSE connections piling up).
deriv(process_resident_memory_bytes[30m]) > 0 and on(instance) checkgate_sse_active_connections < 10

# File descriptors approaching the limit (default soft limit is often 1024 outside containers).
process_open_fds > 0.8 * 1024

# A committed change could not be expanded with its segments (database trouble).
increase(checkgate_resync_requests_total{trigger="segment_read_failed"}[10m]) > 0
```
