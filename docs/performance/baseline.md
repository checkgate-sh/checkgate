---
title: Performance baseline
description: Measured performance of the Checkgate evaluation core and control plane on one developer machine, with the raw results.
---

# Performance baseline

Measured on 2026-10-29 at commit `feat/phase2-production-reliability`
(base `28eb233`), with the harnesses described in
[Benchmark methodology](./benchmark-methodology.md). One machine, two
repetitions of the control-plane run. These are a reference point for
regressions, not capacity guarantees — re-measure on your hardware.

## Environment

| | |
|---|---|
| CPU | Intel Core i5-9400F, 6 cores / 6 threads, 2.9 GHz base; governor `powersave` (could not be changed) |
| Memory | 15 GiB |
| OS | Fedora Linux 44, kernel 7.2.10 |
| Rust | 1.98.0, `release` profile (workspace defaults) |
| PostgreSQL | 16.15 (Docker `postgres:16`), default configuration, `fsync=on`, `synchronous_commit=on`, local disk (LUKS) |
| Redis | 7.4.11 (Docker `redis:7`), default configuration |
| Key crates | axum 0.8.8, hyper 1.9.0, tokio 1.54.1, sqlx 0.8.6, redis 1.2.0, prometheus-client 0.25.1, criterion 0.8.2 |
| Topology | 2 server instances, `DB_MAX_CONNECTIONS=10` each (bootstrap concurrency 5), load generator on the same machine, no CPU pinning |

## Control plane

Raw results: [`results/2026-10-29-loadgen-run1.json`](./results/2026-10-29-loadgen-run1.json),
[`results/2026-10-29-loadgen-run2.json`](./results/2026-10-29-loadgen-run2.json).
Values below are run 1 / run 2.

### SSE connections (2 instances, 101 flags per bootstrap)

| Connections | Ramp-up (all ready) | Time to `ready` p50 / p95 / p99 / max (ms) | Failures | Server RSS per instance, held idle | Server CPU, ramp + 5 s idle (per instance) |
|---|---|---|---|---|---|
| 100 | 0.07 s / 0.06 s | 41 / 59 / 63 / 63 · 36 / 51 / 54 / 60 | 0 / 0 | 16–17 MiB | 0.05 s |
| 1 000 | 0.59 s / 0.57 s | 105 / 135 / 137 / 143 · 104 / 128 / 142 / 163 | 0 / 0 | 28–29 MiB | 0.5 s |
| 5 000 | 2.68 s / 2.62 s | 90 / 153 / 168 / 173 · 103 / 128 / 132 / 137 | 0 / 0 | 80–81 MiB | 2.5 s |

At most 200 handshakes were in flight (a reconnect wave). Memory grows by
about **27 KB per open stream** (64 MiB for 2 450 extra streams per
instance). Bootstraps queue behind `SSE_BOOTSTRAP_CONCURRENCY` (5 here): the
server-side bootstrap mean, which includes that wait, was 75–123 ms.

### Change propagation (write → visible on every stream)

200 sequential `PATCH`es through instance 0, 50 ms apart; one sample per
(update, stream), all on one monotonic clock.

| Connections | Samples | Write → visible p50 / p95 / p99 / max (ms) | After the HTTP response p50 / p95 / p99 (ms) | Fan-out spread, first → last stream p50 / p95 (ms) | Missed |
|---|---|---|---|---|---|
| 100 | 20 000 | 29.4 / 36.4 / 44.0 / 67.1 · 29.5 / 38.3 / 58.9 / 73.1 | 0.0 / 1.2 / 2.4 · 0.0 / 1.1 / 2.1 | 1.4 / 2.9 · 1.5 / 2.5 | 0 |
| 1 000 | 200 000 | 39.5 / 49.2 / 69.0 / 141.7 · 38.8 / 45.9 / 59.7 / 76.7 | 5.1 / 13.3 / 21.4 · 4.7 / 13.0 / 16.4 | 13.4 / 20.4 · 13.7 / 19.2 | 0 |
| 5 000 | 1 000 000 | 52.7 / 94.2 / 140.2 / 200.9 · 55.5 / 96.0 / 113.0 / 127.9 | 25.6 / 54.9 / 71.1 · 27.6 / 60.6 / 70.6 | 44.7 / 75.0 · 46.7 / 75.8 | 0 |

Server CPU while propagating one update every 50 ms: 0.06 / 0.18 / 0.52
cores (instance 0, which also handled the writes) at 100 / 1 000 / 5 000
connections.

**Where the time goes** (server histograms, means over the run):

| Component | Mean |
|---|---|
| `PATCH` handled on the server (auth, PostgreSQL statements and commit, audit log, segment load, publish) | 26.1 ms / 25.0 ms |
| — of which Redis publish | 0.80 ms / 0.75 ms |
| — of which PostgreSQL and the rest | ≈ 25 ms |
| Redis → every instance → SSE write, per stream | the "after the HTTP response" column; grows with connections (fan-out) |
| SDK applying one event (parse + store replace, core bench) | 5.5 µs |

### REST and snapshot

| | p50 / p95 / p99 / max (ms) | Throughput |
|---|---|---|
| `GET /api/environments/{id}/flags?limit=100` (session auth, sequential) | 5.7 / 7.4 / 9.2 / 10.0 · 3.5 / 6.2 / 6.8 / – | – |
| `PATCH /api/environments/{id}/flags/{key}` (sequential) | 28.2 / 33.3 / 35.1 / 40.6 · 23.5 / 31.8 / 36.2 / 47.6 | – |
| `GET /flags/snapshot`, 101 flags, 32 concurrent clients over 2 instances, 10 s | 10.3 / 14.2 / 17.9 / 99.6 · 10.2 / 13.5 / 15.4 / – | 3 016 / 3 082 req/s, 0 errors |

Snapshot reads are capped at 5 concurrent database reads per instance by the
bootstrap slots; the server-side read time averaged 8.2–8.4 ms.

## Evaluation core

Raw output: [`results/2026-10-29-criterion.txt`](./results/2026-10-29-criterion.txt)
(full run; the concurrent group re-run after the harness fix described in the
methodology). Times are Criterion's mean estimate (95 % CI within ±1 %).

| Case | Time per evaluation | Allocations |
|---|---|---|
| Boolean flag, store of 10 / 100 / 1 000 / 10 000 flags | 46.8 / 51.9 / 47.4 / 48.5 ns | 0 |
| Missing flag, 10 … 10 000 flags | 27.8–27.9 ns | 0 |
| Targeting, 1 rule (1 000 flags) | 62.7 ns | 0 |
| Targeting, 10 rules — 9 checked and missed, 1 matched | 259 ns | 0 |
| 50 % rollout | 235 ns | 2 |
| Weighted multivariate (3 variants) | 303 ns | 4 |
| Prerequisite chain of 2 / 3 / 5 flags | 101 / 149 / 254 ns | 0 |
| SDK applying one change event (parse JSON + replace flag) | 5.5 µs | – |

Lookup cost is flat from 10 to 10 000 flags (hash map). Rollout and
multivariate evaluations allocate (the hashing input is built as a string),
which is the main cost difference to a plain boolean.

**Concurrency** (1 000 flags; wall time per evaluation across all threads):

| Reader threads | 1 | 2 | 4 | 6 |
|---|---|---|---|---|
| Readers only | 57.8 ns (17.3 M/s) | 42.4 ns (23.6 M/s) | 26.0 ns (38.4 M/s) | 20.6 ns (48.6 M/s) |
| With one thread replacing flags continuously | 74.2 ns | – | 28.5 ns | – |

A concurrent writer costs little: readers keep an `Arc` of the flag they
evaluate, so replacement never blocks them.

**Memory** (counting allocator): 288 bytes per stored flag at 10 000 flags
(296 at 1 000; small stores carry fixed map overhead). Replacing all 1 000
flags 20 times left 2 KB more retained memory — no growth per replacement.

## Analysis

- **Writes are bound by PostgreSQL, not by fan-out.** About 25 of the 26 ms a
  `PATCH` spends on the server is database work; Redis publish is under 1 ms.
  At 100 connections, nearly all of the 29 ms write-to-visible time is the
  write itself, and most of that is waiting for commits to reach disk (see the
  durability check below).
- **Fan-out cost is linear in connections per instance**: roughly 18 µs per
  stream per event (≈ 45 ms to reach 2 500 streams on one instance, measured
  together with the generator's own receive work on the same CPUs). At 5 000
  connections it is the larger share of propagation latency; more instances
  divide it.
- **Memory per stream (~27 KB)** dominates server memory at scale: 10 000
  streams ≈ 300 MB per instance. `SSE_MAX_CONNECTIONS` (default 10 000) bounds
  it.
- **Reconnect waves are bounded by the bootstrap slots**: 5 000 clients were
  admitted in 2.7 s with no failures and `p99` time-to-ready under 170 ms, while
  the REST API kept its own pool connections.
- **No lagged disconnects, publish failures or missed deliveries** in any run
  (`checkgate_sse_lagged_messages_total`, `checkgate_flag_update_publish_failures_total`).

### Commit durability check

To attribute the write cost, one run (100 connections, 100 updates) was
repeated with PostgreSQL's `synchronous_commit=off` (reset afterwards; raw
result [`results/2026-10-29-loadgen-synchronous-commit-off.json`](./results/2026-10-29-loadgen-synchronous-commit-off.json)):

| | `synchronous_commit=on` (default) | `off` |
|---|---|---|
| `PATCH` p50 / p99 | 28.2 / 35.1 ms | 1.9 / 2.7 ms |
| Write → visible p50 / p99 (100 connections) | 29.4 / 44.0 ms | 14.0 / 24.8 ms |

Waiting for the WAL to reach disk is most of a write's latency on this
machine's encrypted SSD; a write commits twice (the flag change, then its audit
log entry). Faster storage lowers it directly. Checkgate does not change
PostgreSQL's durability setting, and the numbers above were measured with the
default.

## Limitations

- One machine; the load generator shares CPUs with the servers, which inflates
  latency at 5 000 connections. No CPU pinning.
- PostgreSQL and Redis are local containers; production adds network RTT to
  every database statement and to the publish.
- The CPU governor was `powersave`, so absolute times are conservative.
- 10 000+ connection profiles and multi-hour soak runs were not run here;
  the [extended workflow](https://github.com/checkgate-sh/checkgate/blob/dev/.github/workflows/reliability.yml)
  runs the load and soak jobs on CI runners.
- SDK binding overhead (Node N-API, wasm, JSI, Dart FFI) is not measured.
