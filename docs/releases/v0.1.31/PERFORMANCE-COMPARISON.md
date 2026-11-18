# Performance comparison: v0.1.30 → v0.1.31

All numbers below are measured, not projected. v0.1.30 and v0.1.31 ran on the
same host in the same session, alternating, release builds. Raw data is in
[`results/`](results/); the scripts are in `bench/ab/`.

Host: Intel Core i5-8259U (4 cores / 8 threads), 8 GiB, macOS 15, Node
22.23.4, rustc 1.98.0. PostgreSQL 16 and Redis 7 ran in Docker Desktop (VM:
8 vCPU / 3.8 GiB). Host load average during runs was 4–12, from an unrelated
process using about one core; it is logged per run.

## 1. Evaluation latency, Node SDK (public harness, 3 runs each)

`bench/public/sdk/eval.mjs`, unchanged from the published suite: a 1 009-flag
store, 5 user attributes, the public `isEnabled`/`getValue` API with
impressions off. Each run is the median of 100 batch means of 20 000 calls.

| Case | v0.1.30 median (range) | v0.1.31 median (range) | Change |
|---|---|---|---|
| boolean | 6 857 ns (6 856–6 905) | **778 ns** (772–798) | **8.8× faster** |
| targeting, 1 rule | 7 045 ns (6 967–7 079) | 2 149 ns (2 125–2 227) | 3.3× |
| targeting, 10 rules | 7 412 ns (7 376–7 461) | 3 895 ns (3 875–3 967) | 1.9× |
| 50 % rollout | 7 166 ns (7 087–7 203) | **995 ns** (990–1 015) | 7.2× |
| weighted multivariate (`getValue`) | 8 671 ns (8 517–9 077) | 2 240 ns (2 205–2 274) | 3.9× |
| prerequisite chain of 5 | 7 296 ns (7 161–7 554) | 3 213 ns (3 164–3 415) | 2.3× |

The ranges do not overlap in any case. The harness's built-in checks
(identical results, deterministic bucketing across two instances, 4 898/10 000
in a 50 % rollout, 0 network calls while timing) passed in all 6 runs.

**What this means for the docs.** Evaluation through the Node SDK is now
0.8–3.9 µs. It is under a microsecond for flags with no targeting rules, but
not in general, so the docs say "a few microseconds" for Node and
"sub-microsecond" only for the Rust core.

## 2. Evaluation latency, Rust core

The core's hot path is unchanged. The only edit to evaluation is the
prerequisite budget: a `&mut u32` decremented once per prerequisite, which runs
only for flags that have prerequisites. The pinned and exhaustive correctness
tests are new; the Criterion numbers are not re-published.

## 3. Server: `GET /flags/snapshot` (101 flags, 15 271 bytes)

Native A/B, `server/examples/throughput.rs` at concurrency 10, 5 s warm-up,
20 s window, 3 alternating rounds per build, fresh database per run, seeded by
`bench/public/sdk/seed.mjs`. CPU per request is the server's CPU time over
warm-up plus window, divided by requests in that time.

| Metric | v0.1.30 | v0.1.31 | Change |
|---|---|---|---|
| Throughput (median of 3) | 442 req/s (440–448) | **635 req/s** (579–646) | **+44 %** |
| Latency p50 | 21.5 ms | 15.1 ms | −30 % |
| Latency p99 | 38.2 ms (37.0–42.7) | 27.6 ms (27.2–42.5) | −28 % (one v0.1.31 run at 42.5 ms) |
| **Server CPU per request** | **2 634 µs** (2 608–2 638) | **1 757 µs** (1 726–1 794) | **−33 %** |
| Server RSS after the window | 10.0–10.1 MiB | 9.7–10.2 MiB | unchanged |
| Errors | 0 | 0 | — |

**Caveat.** This setup is latency-bound (DB round trips through Docker
Desktop), unlike the published 1 vCPU, CPU-bound configuration, so the req/s
figures are not comparable with the published 504 req/s. CPU per request is
the comparable efficiency measure. An earlier v0.1.31 build that had only the
single-statement snapshot (O3), without O2, measured 519–605 req/s.

## 4. Propagation

Not re-measured. The server sync and stream path is unchanged apart from the
error branch in R2, which never runs in the benchmark, and the SDKs' event
handling is unchanged apart from Flutter framing (R1). The published v0.1.30
results stand: p99 21 ms at 1 client to 90–98 ms at 500–1 000, 0 missed.

## 5. Security and robustness changes with a performance angle

| Change | Before | After |
|---|---|---|
| Pathological prerequisite ladder (10 × 8) | more than 120 s per evaluation (test killed at its timeout) | under 1 ms, fails closed |
| Segment amplification (one write) | an expanded flag of about 180 MB, rebuilt for every bootstrap and snapshot | rejected (422) |
| Argon2 under concurrent logins | blocks async workers; streams and requests stall | blocking pool, one per CPU |

## Regressions

None observed. `/flags/snapshot` now answers **500** where v0.1.30 silently
returned a partial set (an undecodable flag). This is a deliberate correctness
change, not a performance regression.

## Reproduce

```bash
BASE_SDK=../checkgate-base/sdks/nodejs/index.js bench/ab/node-eval-ab.sh
BASE_BIN=/tmp/cg-base-target/release/server bench/ab/snapshot-ab.sh
cargo bench -p checkgate-bench --bench snapshot_decode
```
