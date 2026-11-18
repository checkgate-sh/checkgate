# Performance baseline: v0.1.30

The baseline is the **published v0.1.30 public benchmark**
([`docs/performance/benchmarks.md`](../../performance/benchmarks.md), data in
[`results/v0.1.30/`](../../performance/results/v0.1.30/)). It is kept unchanged.
This page summarizes it and records how the v0.1.31 measurements relate to it.

## Published v0.1.30 results (3 runs each)

Host: Intel Core i5-8259U, 8 GiB, macOS 15.8.1, Docker Desktop (8 vCPU / 3.8 GiB
VM). The server container was limited to 1 vCPU / 512 MiB; load generators ran
on dedicated cpusets.

| Category | Metric | v0.1.30 |
|---|---|---|
| Evaluation, Rust core (Criterion, 1 000-flag store) | boolean / 1 rule / 10 rules / 50 % rollout / multivariate / 5-deep prerequisites | 79 / 114 / 447 / 295 / 436 / 461 ns; 0 allocations except multivariate (1) |
| Evaluation, Node SDK public API (`isEnabled`, 5 attributes) | same six cases | 6.9 / 7.1 / 7.2 / 7.1 / 8.6 / 7.3 µs |
| Server throughput, `GET /flags/snapshot` (101 flags, 15 KB) | req/s at 10 / 100 / 500 / 1 000 concurrent | 504 / 491 / 469 / 501 (CPU-bound at ~0.93 cores) |
| Server memory | RSS, 10 → 1 000 concurrent | 13 → 56 MiB |
| Propagation, PATCH → value visible in the SDK | p50 / p99 at 1 / 100 / 500 / 1 000 clients | 13.3 / 21.2, 19.3 / 39.6, 25.8 / 98.1, 37.7 / 90.3 ms; 0 missed |

## Baseline re-measured for the v0.1.31 comparison

The comparisons in PERFORMANCE-COMPARISON.md re-measure v0.1.30 **in the same
session, on the same host, alternating with the candidate**, so that the
difference is not host drift. Both builds were release builds: a detached
worktree of tag `v0.1.30` with its own `CARGO_TARGET_DIR`, and the candidate.

| Measurement | v0.1.30 published | v0.1.30 re-measured (2026-11-18) | Agreement |
|---|---|---|---|
| Node `isEnabled` boolean | 6 874 ns | 6 857 ns (range 6 856–6 905) | within 0.3 % |
| Node 10 rules | 7 185 ns | 7 412 ns | within 3 % |

The Node harness is the public one (`bench/public/sdk/eval.mjs`), unchanged,
so these numbers are directly comparable with the published ones.

### What differs from the published environment

| | Published (v0.1.30) | v0.1.31 A/B |
|---|---|---|
| Node evaluation | host, Node 22.23.4 | same |
| Snapshot throughput | server in Docker, **1 vCPU quota**, CPU-bound | server **native on macOS, no quota**; PostgreSQL and Redis in Docker, reached through Docker Desktop port forwarding |
| Load | `throughput` example, 4 levels, 60 s windows | same example, concurrency 10, 20 s windows, 5 s warm-up |
| Host load | logged per run | load average 4–12 during runs (an unrelated `top` process used about one core); logged in `results/` |

Because the native snapshot setup is **latency-bound** (each request makes DB
round trips through the Docker Desktop VM), its req/s is not comparable with
the published 1 vCPU figure. The comparable efficiency number is **server CPU
per request**: 2 634 µs for v0.1.30 natively, against ≈1 850 µs implied by the
published 504 req/s at 0.93 cores. The difference comes from the platform and
from counting warm-up.

**The README benchmark graphic still shows v0.1.30.** Refreshing it needs the
full public suite (`bench/public/run.sh all`, about 75 minutes) on the release
commit. This is recommended as a release-finalization step; see
RELEASE-READINESS.md.
