---
title: Public benchmarks (v0.1.31)
description: Evaluation latency, server throughput per CPU and RAM, and flag-update propagation through real SDKs, measured on v0.1.31 and on v0.1.30 with the same harness on the same machine, runs alternated. Every run and the raw data are published.
---

# Public benchmarks (v0.1.31)

Three measurements, each from a harness in this repository that you can rerun:

1. **Evaluation latency**: how long a flag decision takes in-process, in the Rust
   core and through the Node.js SDK's public API.
2. **Server throughput per CPU and RAM**: SDK snapshot reads served by one server
   limited to **1 vCPU and 512 MiB**.
3. **Flag-update propagation**: from a flag change request until real SDK clients
   return the new value from `getValue()`, with up to 1,000 connected clients.

What it costs to keep 100 to 10,000 clients connected, and the same clients
polling instead, is on a separate page:
[Infrastructure cost and efficiency](./cost-efficiency.md).

Measured on 2026-11-18 at commit
[`66cbfec`](https://github.com/checkgate-sh/checkgate/commit/66cbfec6f25c47992e7210150cdd1c25ee770d00)
(the v0.1.31 release candidate; later commits on the branch change CI and
benchmark tooling only). **v0.1.30 was measured again in the same session**,
from its release commit
[`8ef6452`](https://github.com/checkgate-sh/checkgate/commit/8ef6452196ae313f103af9d812e635467348763f),
by `bench/public/compare.sh`: same harness, machine and limits, the two
versions' runs alternated. Every run completed and is included. Raw results:
[`results/v0.1.31/`](https://github.com/checkgate-sh/checkgate/tree/dev/docs/performance/results/v0.1.31)
(v0.1.30 under `baseline-v0.1.30/`).

Comparisons below use only these two same-session sets. The v0.1.30 figures
published on 2026-11-11 came from a different session and are not mixed in.

These numbers describe one laptop under desktop load (see
[Environment](#environment)). They are a reproducible reference point, not a
capacity guarantee: re-measure on your own hardware.

## Summary

| | v0.1.31 | v0.1.30 (same session) | Change |
|---|---|---|---|
| Node SDK `isEnabled()`, 50 % rollout | **1.0 µs** (0.98–1.01) | 7.2 µs (7.16–7.23) | **7.2× faster** |
| Node SDK `isEnabled()`, boolean flag | **0.79 µs** | 7.0 µs | 8.8× faster |
| Rust core, 50 % rollout | 303 ns | 300 ns | unchanged (all six cases within 6 %) |
| Server throughput, 1,000 clients | **591 req/s** (543–645) | 489 req/s (398–502) | +21 % |
| Server throughput, 10 clients | 485 req/s (421–650) | 507 req/s (412–529) | no difference beyond the range |
| Server CPU per snapshot request, 1,000 clients | **1.51 ms** | 1.92 ms | −21 % |
| Server memory | 13 MiB at 10 clients · 59 MiB peak at 1,000 | 14 · 60 MiB | unchanged |
| Propagation p99, 1,000 SDK clients, pooled over 6 runs | **198 ms** | 75 ms | higher: two contended runs (below) |
| Propagation p99, 1,000 SDK clients, median of the 6 run p99s | **84 ms** | 76 ms | +11 % |
| Propagation, deliveries missed | 0 of 1,921,200 | 0 of 1,921,200 | |

Evaluation and throughput are the median of 3 runs (range in brackets);
propagation ran 6 times per version (why 6: [below](#3-flag-update-propagation)).
How results are combined was fixed before the runs
([below](#how-results-are-combined)).

## Environment

| | |
|---|---|
| Host | Intel Core i5-8259U (4 cores / 8 threads, 2.3 GHz), 8 GiB RAM, macOS 15.8.1 |
| Containers | Docker Desktop 29.7.2, Linux VM with 8 vCPU / 3.8 GiB |
| Server | Release build, **1 vCPU (CFS quota), 512 MiB limit**, default settings except per-client rate limits (see below) |
| PostgreSQL | 16.15 (`postgres:16`, pinned digest), default configuration (`fsync` and `synchronous_commit` on), 2 vCPU / 1 GiB |
| Redis | 7.4.11 (`redis:7`, pinned digest), 0.5 vCPU / 256 MiB |
| Load generators | Own containers on dedicated vCPUs 4–7. Server, PostgreSQL and Redis share vCPUs 0–3 |
| Toolchains | Rust 1.98.0 (release profile), Node.js 22.23.4 |

The full record, including image digests, is in
[`environment.json`](https://github.com/checkgate-sh/checkgate/blob/dev/docs/performance/results/v0.1.31/environment.json).

**Conditions on this machine.** The laptop was in desktop use, the 3.8 GiB
Docker VM leaves little RAM on an 8 GiB host (1.7–4.4 GiB of swap in use), and
two idle containers from the integration test suites (a PostgreSQL and a
Redis, CPU p90 under 5 %) shared the Docker VM. `raw/runs.jsonl` records the
host load average and swap before and after every run. The first propagation
repetitions started while the host was still busy from the build (15-minute
load average 15.8 at the start of v0.1.31's first run, 3–4.7 for runs 3–6); see [Propagation](#3-flag-update-propagation).

**Rate limits.** Checkgate's per-client rate limits are keyed by source IP. All
simulated clients here share one IP, so the limits are switched off
(`RATE_LIMIT_*_PER_MINUTE=0`). Admission limits keep their defaults:
`SSE_MAX_CONNECTIONS` (10,000), `SSE_BOOTSTRAP_CONCURRENCY` (5) and
`DB_MAX_CONNECTIONS` (10).

## 1. Evaluation latency

**Rust core** (`bench/benches/evaluation.rs`, Criterion). Per run, the time is
Criterion's printed estimate. Median of 3 runs, with the range.

| Case (1,000-flag store) | v0.1.31 | v0.1.30 | Ops/s, one thread (v0.1.31) | Allocations |
|---|---|---|---|---|
| Boolean flag | 84 ns (83–96) | 80 ns (78–84) | 11.8 M | 0 |
| Targeting, 1 rule | 114 ns (113–120) | 111 ns (108–120) | 8.8 M | 0 |
| Targeting, 10 rules (9 checked and missed) | 441 ns (425–446) | 441 ns (433–446) | 2.3 M | 0 |
| **50 % percentage rollout** | **303 ns** (294–329) | 300 ns (290–360) | 3.3 M | 0 |
| Weighted multivariate (60/30/10) | 456 ns (426–462) | 430 ns (425–439) | 2.2 M | 1 (the returned value) |
| Prerequisite chain of 5 | 424 ns (408–432) | 403 ns (401–408) | 2.4 M | 0 |

The evaluator did not change in v0.1.31. Medians differ by 0–6 %, and every
v0.1.31 range overlaps or touches the v0.1.30 range (the closest: the
prerequisite chain, 408–432 against 401–408 ns). Nothing here is attributed to
the release.

Each Criterion run takes 100 samples, each the mean of many iterations. The
raw files also report the 95 % confidence interval and the p95/p99 of those
sample means. That spread is **not** the latency distribution of single
evaluations: one evaluation is too short to time on its own.

**Node.js SDK** (`bench/public/sdk/eval.mjs`): the same flags and user (5
attributes), through `client.isEnabled()` / `client.getValue()`, so the time
includes crossing the N-API boundary. The client bootstraps over its normal
stream, then disconnects. The timed loops run with `fetch` instrumented and
recorded **0** network calls. Each run takes 100 batches of 20,000 calls;
the table shows the median batch mean, median of 3 runs.

| Case | v0.1.31 | p95 / p99 of batch means | v0.1.30 | Speed-up |
|---|---|---|---|---|
| Boolean flag | 0.79 µs (0.76–0.81) | 0.87 / 0.93 µs | 6.96 µs | 8.8× |
| Targeting, 1 rule | 2.34 µs (2.23–2.40) | 3.18 / 4.65 µs | 7.11 µs | 3.0× |
| Targeting, 10 rules | 4.00 µs (3.88–4.23) | 4.34 / 5.05 µs | 7.51 µs | 1.9× |
| **50 % percentage rollout** | **1.00 µs** (0.98–1.01) | 1.03 / 1.06 µs | 7.16 µs | **7.2×** |
| Weighted multivariate (`getValue`) | 2.18 µs (2.16–2.33) | 2.30 / 2.36 µs | 8.73 µs | 4.0× |
| Prerequisite chain of 5 | 3.18 µs (3.09–3.25) | 3.21 / 3.23 µs | 7.39 µs | 2.3× |

**Why.** v0.1.30 converted the whole attribute object into Rust on every call,
about 1.3 µs per attribute, before looking at the flag. v0.1.31 looks the flag
up first and reads only the attributes its rules reference
([PERFORMANCE-OPTIMIZATIONS.md](https://github.com/checkgate-sh/checkgate/blob/main/docs/releases/v0.1.31/PERFORMANCE-OPTIMIZATIONS.md),
O1). Flags that reference no attribute gain most (boolean, rollout); flags with
many rules gain least, because they read more attributes. Results are unchanged
(360,000 differential calls, 0 differing results), with one documented
difference: a non-string attribute that no rule references no longer throws.

Correctness checks in every run: every case returned its expected value, and
results did not change across calls or between two client instances. A 50 %
rollout over 10,000 user keys enabled 4,898 of them, and a missing flag
evaluated `false`.

## 2. Server throughput per CPU and RAM

`server/examples/throughput.rs` drives `GET /flags/snapshot`, the SDK read path
used for polling fallback. Each request is authenticated with an SDK key, and
the response holds 101 flags (15,271 bytes). The loop is closed: each client
keeps one request in flight. Each level runs 10 s of unrecorded warm-up, then a
60 s window; req/s counts the requests that complete inside the window. Every
response must be a 200 with the expected body.

| Concurrent clients | v0.1.31 req/s, median (range) | v0.1.30 req/s, median (range) | v0.1.31 latency p50 / p95 / p99 (ms) | Errors | Server CPU per request, v0.1.31 / v0.1.30 | Server RSS peak, v0.1.31 / v0.1.30 (MiB) |
|---|---|---|---|---|---|---|
| 10 | 485 (421–650) | 507 (412–529) | 18 / 37 / 74 | 0 | 1.84 / 1.84 ms | 13 / 14 |
| 100 | 546 (473–660) | 512 (426–534) | 166 / 289 / 394 | 0 | 1.62 / 1.83 ms | 18 / 18 |
| 500 | 547 (486–643) | 453 (444–496) | 857 / 1,198 / 1,560 | 0 | 1.63 / 2.07 ms | 39 / 39 |
| **1,000** | **591** (543–645) | 489 (398–502) | 1,647 / 2,060 / 2,433 | 0 | **1.51 / 1.92 ms** | 59 / 60 |

Definitions:

- **Req/s** is measured at a 1-vCPU quota. The server is CPU-bound there, so
  req/s per vCPU is the same number. It is not extrapolated to more cores: more
  cores add database and lock contention that this run does not test.
- **Server CPU per request** = the CPU the server used ÷ req/s.
- **RSS** is sampled from `/proc` once a second; peak is the highest sample.

What the numbers show:

- **v0.1.31 serves each snapshot with less CPU at 100–1,000 clients (−11 % to
  −21 %).** v0.1.31 decodes each stored flag straight into its final type and
  reads flags and segments in one statement (O2, O3). The isolated A/B run for
  those changes measured −33 % CPU per request at 10 clients
  ([PERFORMANCE-COMPARISON.md](https://github.com/checkgate-sh/checkgate/blob/main/docs/releases/v0.1.31/PERFORMANCE-COMPARISON.md)).
  At 10 clients this session shows no difference: both versions' ranges are
  wide (421–650 and 412–529 req/s at the same 0.9 cores of CPU), so how much
  work a vCPU-second buys on this laptop varied more than the change.
- **The server saturates its one vCPU at 10 clients.** More clients add
  queueing delay, not throughput. No errors at any level; 1,000 concurrent
  clients used 59 MiB of the 512 MiB limit.
- **The load generator was not the bottleneck** (0.13–0.18 cores on its own
  vCPUs). PostgreSQL was busy: every snapshot is one database read (p90 up to
  1.15 cores of its 2).

## 3. Flag-update propagation

`bench/public/sdk/propagation.mjs` opens N real `@checkgate/node` clients, at
most 125 per Node.js process and up to 8 processes, each version with its own
SDK. The clients use the SDK's default options, except impression reporting is
off. Every client must bootstrap and evaluate the current value before any
update is sent.

Each update is a `PATCH` that sets the `hot` flag's value to one never used
before. The timer for a client stops when the SDK's own `onChange` callback
fires and its public `getValue()` returns the new value: request, PostgreSQL
commit, Redis publish, SSE delivery, SDK parse and apply, and evaluation.
Updates do not overlap: each waits until every client has seen it (10 s
cut-off), then a 100–150 ms gap. Each profile starts with 5 unrecorded
warm-up updates, then records 200, in every run. All times use
`process.hrtime` (`CLOCK_MONOTONIC`), shared by every process on the kernel.

**Primary metric:** PATCH request sent → new value returned by the SDK,
pooled over every (update, client) sample of all 6 runs.

| SDK clients | v0.1.31 p50 / p95 / **p99** / max (ms) | v0.1.31 p99 per run | v0.1.30 p50 / p95 / **p99** / max (ms) | v0.1.30 p99 per run | Missed | Reconnects |
|---|---|---|---|---|---|---|
| 1 | 11.8 / 18.5 / **59.3** / 272 | 157, 38, 19, 20, 20, 20 | 12.6 / 18.6 / **21.3** / 406 | 134, 20, 20, 19, 21, 19 | 0 / 0 | 0 / 0 |
| 100 | 18.8 / 30.0 / **54.7** / 317 | 98, 72, 33, 29, 34, 31 | 18.5 / 27.8 / **32.8** / 59 | 34, 35, 33, 30, 29, 33 | 0 / 0 | 0 / 0 |
| 500 | 25.4 / 44.4 / **88.6** / 286 | 200, 85, 55, 48, 48, 46 | 25.2 / 39.6 / **49.4** / 131 | 44, 48, 47, 51, 52, 58 | 0 / 0 | 0 / 0 |
| 1,000 | 39.2 / 81.7 / **198.3** / 1,007 | 428, 182, 73, 78, 86, 82 | 37.3 / 61.8 / **75.4** / 129 | 79, 68, 74, 77, 74, 78 | 0 / 0 | 0 / 0 |

| SDK clients | Connect, p99 time to ready (ms) | Server RSS, connections held (MiB) | Server CPU while propagating (cores) | PATCH response p50 (ms) |
|---|---|---|---|---|
| 1,000, v0.1.31 | 2,124 | 42 | 0.25 | 12.8 |
| 1,000, v0.1.30 | 2,112 | 43 | 0.24 | 12.5 |

**Reading the tail.** The p50s are the same for both versions at every client
count. The pooled p99s are not, and the per-run columns show why: v0.1.31's
first two runs were slow at every client count, and its runs 3–6 overlap
v0.1.30's runs at every client count (at 1,000 clients: 73–86 ms against
68–79 ms). What points at the host rather than the release:

- In the same two runs the **PATCH response itself** was slow: its p99 at 1,000
  clients was 210 and 32 ms, against 20–28 ms in every other run of either
  version. The write path (HTTP, PostgreSQL commit) does not involve the SDKs,
  and RC2 did not change it.
- The first runs started with the host still loaded from the image builds
  (15-minute load average 15.8 and 7.5, against 2.9–4.7 for runs 3–6).
  v0.1.30's first run, at a load of 11, has the same kind of outlier at 1
  client (134 ms).
- The SDK processes' CPU at 1,000 clients was 1.28 and 1.05 cores in those two
  runs, against 0.84–0.89 in runs 3–6 and 0.82–0.91 for v0.1.30.
- Two isolation runs made while investigating, outside the published set: the
  two Node SDKs against one scripted SSE server (no network, no database), and
  the two servers driven by the same SDK. Neither showed a difference.
- 0 reconnects and 0 clients polling in every run, so the new liveness deadline
  did not replace any stream.

**Why 6 runs.** `compare.sh` ran 3 per version. After v0.1.31's first two came
out slow, 3 more per version were added, alternated the same way
(`run.sh propagation` with `REP_START=4…6`, one run per version in turn, on
the images `compare.sh` had built), on the quieter host, before any run was dropped or
re-aggregated; none was. That decision was made after seeing the first
results, so it is stated here. All six runs are kept and the pooled p99 is
published as measured, because the aggregation rule was fixed before the runs. The median of the run p99s
(84 against 76 ms at 1,000 clients) is shown next to it. A rerun on a
dedicated Linux host is the way to settle the remaining 11 %; it is listed as
a follow-up in RC2-RELEASE-READINESS.md.

Also from these runs:

- **Every update reached every client**: 1,921,200 deliveries per version,
  0 missed, 0 reconnects. At 1,000 clients, 99.998 % of v0.1.31's deliveries
  (all but about 24) arrived within 1 s; v0.1.30's all did.
- **The harness is part of the measured tail.** Real deployments run one SDK
  per process; here up to 125 SDK instances share a Node.js process and handle
  each update one after another (about 0.9 cores between them at 1,000
  clients). At high client counts these figures are an upper bound for the
  server's share.

## Reproduce

```bash
bench/public/run.sh all            # about 75 minutes
bench/public/run.sh eval           # or one part at a time
bench/public/run.sh server
bench/public/run.sh propagation
bench/public/run.sh cost           # connected-client cost, about 50 minutes
bench/public/run.sh summarize      # rebuilds the summaries and the README graphics
python3 bench/public/scripts/validate.py
```

Against an earlier version, same machine and limits, runs alternated:

```bash
bench/public/compare.sh v0.1.30
```

Requirements: Docker with Compose v2 (a VM with 8 vCPUs for the default CPU
placement; see `bench/public/compose.yml` to change it), the pinned Rust
toolchain, Node.js 22 and Python 3. [`bench/public/README.md`](https://github.com/checkgate-sh/checkgate/blob/dev/bench/public/README.md)
describes the datasets, the containers and what each harness checks.

Results land in `docs/performance/results/v0.1.31/`:

- `raw/` holds every run: Criterion output, per-run JSON, per-container CPU and
  memory, and `runs.jsonl` with time, host load and swap per run.
- `raw/failed/` keeps any run that fails (none this time).
- `raw/superseded-*/` keeps complete runs replaced after a harness fix, with
  the reason (this time: the first polling profiles of the cost suite).
- `summary.json` and `cost-summary.json` are generated from `raw/`, and the
  README graphics from those. CI checks that all of them are current.

### How results are combined

Fixed before the runs:

- Evaluation and throughput use the median of 3 runs, shown with the range.
- Propagation percentiles are pooled over all samples of all runs.
- The README graphic shows the 50 % rollout case through the Node SDK; the
  throughput level with the highest median among levels with zero errors, and
  the CPU per request at that level, each compared with v0.1.30 at the same
  level; and p99 propagation at the largest client count that completed in
  every run, with the median of the run p99s beside it.

## Limitations

- **One machine, a laptop in use.** CPU time competes with the desktop, and
  swap was in use throughout. Absolute numbers are conservative, and run-to-run
  variance is higher than on a dedicated host: it is large enough here to hide
  a 20 % throughput change at 10 clients.
- **Containers on macOS.** PostgreSQL's commits go to the Docker VM's virtual
  disk, and the network is the VM's internal bridge. Production adds real disk
  and network round trips to each write and each database read.
- **One server instance.** Multi-instance fan-out through Redis is not
  measured here. The [October 2026 baseline](./baseline.md) ran two instances,
  on different hardware.
- **Saturation was measured only at 1 vCPU**, and propagation only up to 1,000
  SDK clients (raw connection costs up to 10,000 are on the
  [cost page](./cost-efficiency.md)).
- **Snapshot reads only.** Throughput covers `GET /flags/snapshot`, the SDK
  read path. Administrative writes run once per update in the propagation
  benchmark; they were not load-tested.
- **Only the Node.js SDK is measured.** The web (WebAssembly), React Native and
  Flutter SDKs share the Rust core but have their own boundaries, and they were
  not benchmarked.
- The Node SDK numbers exclude impression reporting (on by default), which
  buffers every evaluation and sends batches over the network.

Earlier results: [v0.1.30, measured 2026-11-11](https://github.com/checkgate-sh/checkgate/tree/dev/docs/performance/results/v0.1.30)
(a different session; not compared here). See also:
[Benchmark methodology](./benchmark-methodology.md) for the Criterion and
`loadgen` harnesses, and the [October 2026 baseline](./baseline.md).
