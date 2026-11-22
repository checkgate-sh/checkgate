# Release benchmarks on a dedicated Linux host

`v0.2.0-linux.sh` compares a baseline release with a candidate commit on one
machine, with everything but the code held constant. It is the performance
evidence for the v0.2.0 release gate; the public suite (`bench/public/`)
measures one build and is not an A/B.

```bash
# 1. Validate the host only (no builds, containers or files):
scripts/release-benchmarks/v0.2.0-linux.sh --check --candidate <commit-sha>

# 2. Full run (about 1.5 h with the defaults on an 8-core host):
scripts/release-benchmarks/v0.2.0-linux.sh \
  --baseline v0.1.31 --candidate <commit-sha> --output ./benchmark-results/v0.2.0

# Optional: a 10-minute smoke run of the whole workflow (verdicts are
# INCONCLUSIVE by design; it proves the workflow, not performance):
scripts/release-benchmarks/v0.2.0-linux.sh --quick --candidate <commit-sha> --output ./benchmark-results/smoke
```

`--help` lists every option.

## What is held constant

| | |
|---|---|
| Revisions | Both refs resolve to commit SHAs before anything runs; branch names are refused. v0.1.31 is checked against the published release commit |
| Builds | One `git worktree` per side under `<output>/work`, each with its own `CARGO_TARGET_DIR`, release profile, `--locked`. Both refs pin Rust 1.98.0; `RUSTUP_AUTO_INSTALL=0`, so a missing toolchain is an error, not a download |
| Core workload | `lib/core_ab.rs` is copied into both worktrees and compiled against each core: the same file, using only API both releases have. Each release's own Criterion suite differs, so it is not used for the A/B |
| Node SDK workload | The candidate's `bench/public/sdk/eval.mjs` run against each side's built `@checkgate/node` |
| Server workload | The candidate's `throughput` example (snapshot), `seed.mjs`, `propagation.mjs` and Node SDK (as SSE client) for both servers: only the server binary differs |
| Data stores | One PostgreSQL 16 and one Redis 7 container (CI's pinned digests), created by the run, PostgreSQL on tmpfs by default; a fresh database and an empty Redis for every run |
| CPU | Server, load generators and data stores on disjoint CPU sets (`taskset`, `--cpuset-cpus`); microbenchmarks on one server CPU while nothing else runs |
| Configuration | Identical server environment, rate limits off, `RUST_LOG=warn` |

## Method

- Per suite, one unrecorded warm-up run per side, then `--rounds` rounds of
  **Baseline → Candidate → Candidate → Baseline** (default 3 rounds: 6 runs
  per side). Compilation happens before any measurement.
- Before every run the driver waits (up to 90 s) until the host is idle:
  less than 10 % of all CPUs busy over 3 s, measured from `/proc/stat`.
- A sampler records the measured process and the host once a second: CPU,
  RSS and its high-water mark, threads, file descriptors, established
  connections, host busy and steal share, swap.
- SSE runs also open 25 raw `/stream` connections (`lib/sse_audit.mjs`) that
  count duplicated and out-of-order deliveries and unexpected disconnects,
  which the SDK-level harness cannot see.

**Invalid runs** (excluded from statistics, listed in `summary.md`, logs kept
in `failed/`): the harness exited non-zero, the host stayed busy before the
run, CPU steal above 5 %, swap grew by more than 64 MiB, or the server exited.
A server that exits also fails the stability gate, so excluding the run never
hides a crash.

**Verdicts** (`lib/analyze.py`): per metric, ratio of medians (candidate /
baseline) with a 95 % percentile-bootstrap confidence interval over runs
(fixed seed). With the regression threshold *t* (default 10 %): PASS when the
interval excludes a regression larger than *t*, FAIL when the whole interval
is a regression larger than *t*, INCONCLUSIVE otherwise — and also when a side
has fewer than 3 valid runs or a run-to-run coefficient of variation above
*t*. Missed, duplicated or out-of-order updates, unexpected reconnects or
disconnects, HTTP errors and server exits must be 0 for the candidate. Any
gating FAIL makes the overall verdict FAIL; any gating INCONCLUSIVE makes it
INCONCLUSIVE.

## Output

```
<output>/
  summary.md, summary.json   verdicts per metric, suite and overall
  environment.json           SHAs, toolchains, host (CPU, memory, kernel,
                             governor, SMT, THP, limits), tools, check results
  raw/runs.jsonl             one record per run: order, validity, host load
  raw/<suite>/*.json         harness results; *.samples.jsonl sampler data;
                             *.audit.json SSE delivery audit
  logs/                      driver, builds, checks, per-run harness and server logs
  failed/                    logs of invalid runs
  warmup/                    the unrecorded warm-up runs
```

`--analyze-only <output>` recomputes the summary from `raw/` (for example
with another `--threshold`) without running anything.

## Cleanup and safety

The run creates two worktrees and two build directories under `--work-dir`,
and two containers named `cgrb-<run id>-pg` / `cgrb-<run id>-redis` with the
label `checkgate.release-bench.run=<run id>`. On exit (also on Ctrl-C) it
removes exactly those, unless `--keep-work`. It never stops, removes or
prunes anything else, and changes no host setting: `--check` reports the CPU
governor, swap, competing containers and processes, and leaves fixing them
to you.

## Handing results back

Archive the output directory (`tar czf v0.2.0-linux-bench.tgz -C benchmark-results v0.2.0`)
and attach it to the release PR, or copy it to
`internal_docs/releases/v0.2.0/results/linux-bench/`. `summary.json` carries
the commit SHAs, so results can be matched to the candidate they measured.
