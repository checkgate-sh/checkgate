---
title: Public benchmarks (v0.1.30)
description: Evaluation latency, server throughput per CPU and RAM, and flag-update propagation through real SDKs, measured on v0.1.30 with a reproducible harness. Every run and the raw data are published.
---

# Public benchmarks (v0.1.30)

Three measurements, each from a harness in this repository that you can rerun:

1. **Evaluation latency**: how long a flag decision takes in-process, in the Rust
   core and through the Node.js SDK's public API.
2. **Server throughput per CPU and RAM**: SDK snapshot reads served by one server
   limited to **1 vCPU and 512 MiB**.
3. **Flag-update propagation**: from a flag change request until real SDK clients
   return the new value from `getValue()`, with up to 1,000 connected clients.

Measured on 2026-11-11 at commit
[`b5d7556`](https://github.com/checkgate-sh/checkgate/commit/b5d755683937dfff0e088550e6518ffcbce1954b)
(the v0.1.30 release candidate). Each benchmark ran **3 times**, and every run
completed and is included. Raw results:
[`results/v0.1.30/`](https://github.com/checkgate-sh/checkgate/tree/dev/docs/performance/results/v0.1.30).

An earlier set of 3 throughput runs was replaced after a fix to how the harness counts
requests. It counted only requests that started inside the window as well as finished
there, which undercounted by up to 3.5 %. That set is kept, with the reason, in
`raw/superseded-throughput-window-rule/`.

These numbers describe one laptop under real desktop load (see
[Environment](#environment)). They're a reproducible reference point, not a
capacity guarantee, so re-measure on your own hardware.

## Summary

| | Result | Conditions |
|---|---|---|
| Evaluation, Rust core | **295 ns** (50 % rollout) · 79 ns (boolean) | 1,000-flag store, one thread, 0 allocations |
| Evaluation, Node SDK `isEnabled()` | **7.1 µs** (50 % rollout) · 6.9 µs (boolean) | Same flags, user with 5 attributes, N-API boundary included |
| Server throughput | **504 req/s** (range 423–550) | `GET /flags/snapshot`, 101 flags (15 KB), 10 clients, 1 vCPU / 512 MiB limit, 0 errors |
| Server memory | **13 MiB** RSS at 10 clients · 58 MiB peak at 1,000 | Same runs |
| Propagation p99 | **90 ms** at 1,000 SDK clients · 40 ms at 100 · 21 ms at 1 | 600,000 deliveries at 1,000 clients, **0 missed**, 0 reconnects |

## Environment

| | |
|---|---|
| Host | Intel Core i5-8259U (4 cores / 8 threads, 2.3 GHz), 8 GiB RAM, macOS 15.8.1 |
| Containers | Docker Desktop 29.7.2, Linux VM with 8 vCPU / 3.8 GiB |
| Server | Release build of `b5d7556`, **1 vCPU (CFS quota), 512 MiB limit**, default settings except per-client rate limits (see below) |
| PostgreSQL | 16.15 (`postgres:16`, pinned digest), default configuration (`fsync` and `synchronous_commit` on), 2 vCPU / 1 GiB |
| Redis | 7.4.11 (`redis:7`, pinned digest), 0.5 vCPU / 256 MiB |
| Load generators | Own containers on dedicated vCPUs 4–7. Server, PostgreSQL and Redis share vCPUs 0–3 |
| Toolchains | Rust 1.98.0 (release profile), Node.js 22.23.4 |

The full record, including image digests, is in
[`environment.json`](https://github.com/checkgate-sh/checkgate/blob/dev/docs/performance/results/v0.1.30/environment.json).

**Conditions on this machine.** The laptop was in normal desktop use during every run
(editor, browser, other background apps), and the 3.8 GiB Docker VM leaves little RAM
on an 8 GiB host: 1.8–4.6 GiB of swap was in use. `raw/runs.jsonl` records the host load
average and swap before and after each run. Both slow the CPU work and add
occasional stalls, so expect lower latency and less variance on a dedicated machine.

**Rate limits.** Checkgate's per-client rate limits are keyed by source IP. All
simulated clients here share one IP, so the limits are switched off
(`RATE_LIMIT_*_PER_MINUTE=0`). Admission limits keep their defaults:
`SSE_MAX_CONNECTIONS` (10,000), `SSE_BOOTSTRAP_CONCURRENCY` (5) and
`DB_MAX_CONNECTIONS` (10).

## 1. Evaluation latency

**Rust core** (`bench/benches/evaluation.rs`, Criterion). Per run, the time is
Criterion's printed estimate. The table shows the median of 3 runs, the range,
and single-thread throughput (1 / time).

| Case (1,000-flag store) | Time per evaluation | Ops/s, one thread | Allocations |
|---|---|---|---|
| Boolean flag | 79 ns (78–79) | 12.7 M | 0 |
| Targeting, 1 rule | 114 ns (110–116) | 8.8 M | 0 |
| Targeting, 10 rules (9 checked and missed) | 447 ns (434–451) | 2.2 M | 0 |
| **50 % percentage rollout** | **295 ns** (293–332) | 3.4 M | 0 |
| Weighted multivariate (60/30/10) | 436 ns (430–509) | 2.3 M | 1 (the returned value) |
| Prerequisite chain of 5 | 461 ns (410–605) | 2.2 M | 0 |

Each Criterion run takes 100 samples, and each sample is the mean of many
iterations. The raw files also report the 95 % confidence interval and the
p95/p99 of those sample means (the per-sample spread). That spread is **not**
the latency distribution of single evaluations: one evaluation is too short to
time on its own.

Also from these runs: a stored flag uses about 290 bytes at 1,000–10,000 flags.
Replacing all 1,000 flags 20 times grew retained memory by 1–7 KB in total, depending on the run. Four
threads evaluate at 39–45 ns of wall time per evaluation combined
(about 24 M/s), and eight threads at 29–32 ns.

**Node.js SDK** (`bench/public/sdk/eval.mjs`): the same flags and user, through
`client.isEnabled()` / `client.getValue()`, so this time includes crossing the
N-API boundary. The client bootstraps over its normal stream code path, then
disconnects. The timed loops run with `fetch` instrumented, and recorded **0**
network calls. Each run takes 100 batches of 20,000 calls; the table shows the
median batch mean, median of 3 runs.

| Case | Time per call | p99 of batch means | Calls/s |
|---|---|---|---|
| Boolean flag | 6.9 µs (6.9–7.1) | 7.4 µs | 145 k |
| Targeting, 1 rule | 7.1 µs (7.0–7.3) | 7.7 µs | 142 k |
| Targeting, 10 rules | 7.4 µs (7.2–7.6) | 8.4 µs | 136 k |
| 50 % percentage rollout | 7.1 µs (7.0–7.3) | 8.3 µs | 140 k |
| Weighted multivariate (`getValue`) | 8.7 µs (8.5–8.7) | 9.9 µs | 115 k |
| Prerequisite chain of 5 | 7.2 µs (7.2–7.4) | 8.0 µs | 138 k |

Correctness checks in every run: every case returned its expected value, and
results didn't change across calls or between two independent client instances.
A 50 % rollout over 10,000 user keys enabled 4,898 of them, and a missing flag
evaluated `false`.

**Where the Node SDK's time goes.** Almost all of it is spent converting the
arguments, not evaluating. A separate probe of the native binding on the same
machine took about 1 µs per call with an empty attribute object, and about 1.3 µs
more for each attribute. That conversion happens before the flag lookup, so it
costs the same whether or not the flag uses targeting rules, and even for a
missing flag. The evaluation logic is the Rust core's 79–461 ns.

## 2. Server throughput per CPU and RAM

`server/examples/throughput.rs` drives `GET /flags/snapshot`, the SDK read path
used for polling fallback. Each request is authenticated with an SDK key, and the
response holds 101 flags (15,271 bytes). The loop is closed: each client keeps one
request in flight. Each level runs 10 s of unrecorded warm-up, then a 60 s window,
and req/s counts the requests that complete inside the window. Every response
must be a 200 with the expected body.

| Concurrent clients | Successful req/s, median (range) | Latency p50 / p95 / p99 (ms) | Errors | Server CPU, mean / peak (cores) | Server RSS, median / peak (MiB) | Requests per server CPU-second | Generator CPU (cores) |
|---|---|---|---|---|---|---|---|
| 10 | **504** (423–550) | 19 / 27 / 35 | 0 | 0.93 / 0.97 | 13 / 13 | 541 | 0.09 |
| 100 | 491 (470–545) | 196 / 249 / 320 | 0 | 0.94 / 0.97 | 18 / 18 | 525 | 0.11 |
| 500 | 469 (446–507) | 1,058 / 1,156 / 1,277 | 0 | 0.94 / 0.96 | 33 / 39 | 501 | 0.13 |
| 1,000 | 501 (458–506) | 1,955 / 2,296 / 2,709 | 0 | 0.94 / 0.97 | 56 / 58 | 536 | 0.11 |

Definitions:

- **Req/s** is measured at a 1-vCPU quota. Since the server is CPU-bound at that
  limit, req/s per vCPU is the same number. This is a measurement at one vCPU, not
  an extrapolation to more cores: more cores add database and lock contention that
  this run doesn't test.
- **Requests per server CPU-second** = req/s ÷ the CPU the server actually used.
- **RSS** is sampled from `/proc` once a second. Peak is the highest sample.

What the numbers show:

- **The server saturates its one vCPU at 10 clients.** More clients add queueing
  delay, not throughput: latency grows roughly in proportion to concurrency, which
  is what a closed loop at saturation does. There were **no errors at any level**,
  and 1,000 concurrent clients stayed far inside the memory limit (58 MiB of 512).
- **Each snapshot costs about 1.9–2.0 ms of server CPU.** A profile of the same
  server under load (macOS, used only for attribution) put most of the server's
  on-CPU time in JSON handling and the memory allocator. Each request parses every
  stored flag into a generic JSON value, converts it to a flag, and serialises it
  again. sqlx frames were about 5 % of the samples. JSON handling is the dominant cost on this path.
- **The load generator was not the bottleneck**: it used 0.1 core on dedicated
  vCPUs. PostgreSQL ran at about 0.8 cores of its 2 (p90 0.89–0.93 per run, over the whole run).
- **Variance between runs is large (423–550 req/s at 10 clients).** Server CPU
  was 0.93 cores in every run. What changed is how much work one vCPU of time
  buys on a shared laptop. That's why the range is shown with every median.

## 3. Flag-update propagation

`bench/public/sdk/propagation.mjs` opens N real `@checkgate/node` clients, at most
125 per Node.js process and up to 8 processes. The clients use the SDK's default
options, except impression reporting is off. Every client must bootstrap and
evaluate the current value before any update is sent.

Each update is a `PATCH` that sets the `hot` flag's value to one never used before.
The timer for a client stops when the SDK's own `onChange` callback fires and its
public `getValue()` returns the new value. That covers request, PostgreSQL commit,
Redis publish, SSE delivery, SDK parse and apply, and evaluation. Updates don't
overlap: each waits until every client has seen it, with a 10 s cut-off, then a
100–150 ms gap. Each profile starts with 5 unrecorded warm-up updates, then
records 200, in every one of 3 runs.

All times use `process.hrtime` (`CLOCK_MONOTONIC`), which every process on the
same kernel shares, so they need no clock synchronisation.

**Primary metric:** PATCH request sent → new value returned by the SDK. The
percentiles below are pooled over every (update, client) sample of all 3 runs.

| SDK clients | Samples | p50 / p95 / **p99** / max (ms) | p99 per run (ms) | p99 after the PATCH response (ms) | Within 1 s | Missed | Reconnects |
|---|---|---|---|---|---|---|---|
| 1 | 600 | 13.3 / 19.9 / **21.2** / 83 | 21 / 21 / 21 | 0 | 100 % | 0 | 0 |
| 100 | 60,000 | 19.3 / 30.1 / **39.6** / 85 | 42 / 35 / 40 | 14 | 100 % | 0 | 0 |
| 500 | 300,000 | 25.8 / 43.0 / **98.1** / 365 | 144 / 47 / 83 | 44 | 100 % | 0 | 0 |
| 1,000 | 600,000 | 37.7 / 63.2 / **90.3** / 871 | 112 / 77 / 72 | 58 | 100 % | 0 | 0 |

| SDK clients | Connect, p99 time to ready (ms) | Server RSS, connections held (MiB) | Server CPU while propagating (cores) | SDK processes' CPU (cores) | PATCH response p50 (ms) |
|---|---|---|---|---|---|
| 1 | 110 | 12 | 0.04 | 0.01 | 15 |
| 100 | 501 | 16 | 0.05 | 0.08 | 14 |
| 500 | 1,336 | 27 | 0.14 | 0.51 | 13 |
| 1,000 | 2,182 | 43 | 0.23 | 0.95 | 14 |

Reading the table:

- **Every update reached every client.** 960,600 deliveries across all profiles and
  runs, 0 missed, 0 reconnects, and no client fell back to polling.
- **"After the PATCH response"** subtracts the write's own response time. With one
  client, 91 % of updates were already visible before the PATCH response arrived:
  the server publishes before it responds. With more clients, the remaining time is
  fan-out plus the SDKs' own work.
- **p99 is not monotonic in client count**: 500 clients came out higher than 1,000.
  Run 1 at 500 clients alone had a p99 of 144 ms. One slow update delays every
  client at once, so a few host stalls move p99 a lot. The p99 of each run is shown
  so that spread is visible.
- **The harness is part of the measured tail.** Real deployments run one SDK per
  process, but here up to 125 SDK instances share a Node.js process and handle
  each update one after another. At 1,000 clients those processes used up to
  0.95 cores between them. Packing more clients into fewer processes lengthened the
  tail in trials, so at high client counts these figures are an upper bound for
  the server's share.
- The server held 1,000 open streams in 43 MiB, and used 0.23 cores while updates
  went out back to back (one roughly every 0.2 s).

## Reproduce

```bash
bench/public/run.sh all            # about 75 minutes
bench/public/run.sh eval           # or one part at a time
bench/public/run.sh server
bench/public/run.sh propagation
bench/public/run.sh summarize      # rebuilds summary.json and the README graphic
python3 bench/public/scripts/validate.py
```

Requirements: Docker with Compose v2 (a VM with 8 vCPUs for the default CPU
placement; see `bench/public/compose.yml` to change it), the pinned Rust
toolchain, Node.js 22 and Python 3. [`bench/public/README.md`](https://github.com/checkgate-sh/checkgate/blob/dev/bench/public/README.md)
describes the datasets, the containers and what each harness checks.

Results land in `docs/performance/results/v0.1.30/`:

- `raw/` holds every run: Criterion output, per-run JSON, per-container CPU and
  memory, and `runs.jsonl` with time, host load and swap per run.
- `raw/failed/` keeps any run that fails.
- `summary.json` is generated from `raw/`, and the README graphic is generated from
  `summary.json`. CI checks that both are current.

**How results are combined** (fixed before the runs):

- Evaluation and throughput use the median of 3 runs, shown with the range.
- Propagation percentiles are pooled over all samples of all runs.
- The README graphic shows:
  - the 50 % rollout case;
  - the throughput level with the highest median among levels with zero errors;
  - p99 at the largest client count that completed in every run.

## Limitations

- **One machine, a laptop in use.** CPU time competes with the desktop, and swap was in
  use throughout. Absolute numbers are conservative, and run-to-run variance is
  higher than on a dedicated host.
- **Containers on macOS.** PostgreSQL's commits go to the Docker VM's virtual disk,
  and the network is the VM's internal bridge. Production adds real disk and network
  round trips to each write and to each database read.
- **One server instance.** Multi-instance fan-out through Redis is not measured here.
  The [October 2026 baseline](./baseline.md) ran two instances, on different hardware.
- **Saturation was measured only at 1 vCPU.** Higher CPU limits and more than 1,000
  concurrent clients were not tested. Above 1,000 SDK clients, this machine couldn't
  separate the server's behaviour from the harness's.
- **Snapshot reads only.** Throughput covers `GET /flags/snapshot`, the SDK read path.
  Administrative writes run once per update in the propagation benchmark; they were not
  load-tested.
- **Only the Node.js SDK is measured.** The web (WebAssembly), React Native and
  Flutter SDKs share the Rust core but have their own boundaries, and they were not
  benchmarked.
- The Node SDK numbers exclude impression reporting (on by default), which buffers
  every evaluation and sends batches over the network.

See also: [Benchmark methodology](./benchmark-methodology.md) for the Criterion and
`loadgen` harnesses, and the [October 2026 baseline](./baseline.md).
