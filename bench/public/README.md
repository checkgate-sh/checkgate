# Public benchmark suite

The harness behind the performance section of the root README and
[docs/performance/benchmarks.md](../../docs/performance/benchmarks.md). It measures three things:

| Metric | Harness | Where it runs |
|---|---|---|
| Evaluation latency, Rust core | `bench/benches/evaluation.rs` (Criterion; the existing bench, unchanged) | host |
| Evaluation latency, Node SDK public API | `bench/public/sdk/eval.mjs` | host |
| Server throughput per CPU and RAM | `server/examples/throughput.rs` against a resource-limited server | Docker Compose |
| Flag-update propagation | `bench/public/sdk/propagation.mjs` with real `@checkgate/node` clients | Docker Compose |

## Requirements

Docker with Compose v2, a Rust toolchain (pinned by `rust-toolchain.toml`), Node 22,
and Python 3 (standard library only). The default CPU placement needs a Docker VM with
8 vCPUs. On a smaller one, set `BENCH_SYSTEM_CPUSET` and `BENCH_GENERATOR_CPUSET` (for
example `0-1` and `2-3`). Every container at its limit needs about 3.5 GiB.

## Run

```bash
bench/public/run.sh all            # everything, about 75 minutes
bench/public/run.sh eval           # Criterion + Node SDK evaluation, 3 runs each
bench/public/run.sh server         # throughput, 3 fresh stacks
bench/public/run.sh propagation    # propagation, 3 fresh stacks
bench/public/run.sh environment    # environment.json
bench/public/run.sh summarize      # summary.json and the README graphic
python3 bench/public/scripts/validate.py   # what CI checks
```

Results go to `docs/performance/results/v0.1.30/` (set `OUT` to change it):

```text
environment.json          host, Docker VM, toolchains, commit, image digests, limits
summary.json              aggregated results and the headline values (generated)
raw/runs.jsonl            one line per run: phase, time, host load, ok/failed
raw/criterion-runN.txt    Criterion output
raw/evaluation-rust-runN.json, evaluation-node-runN.json
raw/throughput-runN.json, throughput-runN.containers.json
raw/propagation-runN.json, propagation-runN.containers.json
raw/failed/               anything from a run that failed (kept, never deleted)
```

The README graphic, `assets/benchmarks/performance-{light,dark}.svg`, is generated
from `summary.json` by `scripts/render_svg.py`. Don't edit the SVGs by hand. CI
re-renders them and fails if they differ.

## Environment (`compose.yml`)

| Service | Image | Limit |
|---|---|---|
| `server` | `bench/public/Dockerfile` target `server` (release build of this checkout) | `BENCH_SERVER_CPUS` (1) vCPU quota, `BENCH_SERVER_MEMORY` (512m); vCPUs 0–3 |
| `postgres` | `postgres:16`, pinned digest, default configuration | 2 vCPU, 1 GiB; vCPUs 0–3 |
| `redis` | `redis:7`, pinned digest | 0.5 vCPU, 256 MiB; vCPUs 0–3 |
| `loadgen` | target `loadgen` (`throughput` example) | dedicated vCPUs 4–7, no quota, 1 GiB |
| `sdk` | target `sdk` (Node 22 + `@checkgate/node` built for Linux) | dedicated vCPUs 4–7, no quota, 1.5 GiB |

The load generators share the server's PID namespace (`pid: service:server`) and read
the server's CPU time and RSS from `/proc/1`. The server's limit is a CFS quota: a
"1 vCPU" server can use one CPU's worth of time per period, on any of vCPUs 0–3. The
generators get whole vCPUs instead of a quota. In trials, a quota throttled the SDK
processes in the middle of an update burst, and that showed up as false propagation
latency (p99 rose from 0.25 s to 2.2 s as processes were added).

The server runs with its default settings, with one exception: per-client rate
limits (`RATE_LIMIT_*_PER_MINUTE=0`). They are keyed by source IP, and every
simulated client here comes from one IP. Admission limits (`SSE_MAX_CONNECTIONS`,
`SSE_BOOTSTRAP_CONCURRENCY`, `DB_MAX_CONNECTIONS`, the bootstrap queue timeout)
keep their defaults.

## Datasets

`sdk/fixtures.mjs` defines both datasets in code, deterministically:

- **Server dataset**: 100 flags (a quarter each: boolean, 25 % rollout, one targeting
  rule, two-way multivariate) plus `hot`, a string flag. `sdk/seed.mjs` creates them
  through the public setup and flag APIs into an empty database. `GET /flags/snapshot`
  returns all 101.
- **Evaluation dataset**: 1 000 boolean flags plus the special flags and the user of
  `bench/benches/evaluation.rs`, so the Node SDK and the Rust core evaluate the same
  thing.

## What each harness guarantees

- **`throughput`**: closed loop, one request in flight per client. `WARMUP_SECS` of
  unrecorded load, then a `WINDOW_SECS` window. Requests that complete inside the window
  count (throughput = completions per second). The first snapshot is parsed and its flag count checked. Every later
  response must be 200 with the same body length, or it counts as an error. It records
  the server's CPU and RSS each second, and its own CPU, so a saturated generator shows.
- **`propagation`**: N real SDK clients, at most 125 per worker process and up to 8
  processes. Every client must bootstrap and evaluate the current value before any
  update. Each profile begins with 5 unrecorded warm-up updates, because the first
  writes after connecting take a cold path. Updates are sequential:
  each sets `hot` to a value never used before, and the next waits until every client
  has observed it (or 10 s). "Observed" means the SDK's public `getValue()` returned the
  new value, inside the SDK's `onChange` callback. Times come from `process.hrtime`
  (`CLOCK_MONOTONIC`, shared by every process on the kernel), so no clocks need syncing.
  Missed updates, reconnects and clients that fall back to polling are counted.
- **`eval.mjs`**: the client bootstraps from a local stub stream, then disconnects. The
  timed loop runs with `fetch` instrumented, and any network call fails the run. Results
  are checked against expected values and across two client instances, and a 50 %
  rollout over 10 000 users must split within 47–53 %.
