# Public benchmark results: v0.1.31 RC2 against v0.1.30

The public benchmark suite (`bench/public`) run on the release candidate and on
v0.1.30 under identical conditions. Full method, every table and the raw
data: [docs/performance/benchmarks.md](../../performance/benchmarks.md).

## How the comparison was made

- `bench/public/compare.sh v0.1.30` builds both versions' images, then runs
  the **same harness** (this tree's `bench/public`, copied into a worktree of
  `v0.1.30`) against each, on the same host, with the same container limits,
  alternating the versions repetition by repetition so drift on the host hits
  both alike.
- Candidate: `66cbfec` (RC2; later commits change CI and benchmark tooling
  only). Baseline: `8ef6452` (the v0.1.30 release commit). Both builds record
  that the measured source matched the commit.
- 3 repetitions of evaluation and throughput per version; propagation 3, then
  3 more per version (reason below). Every run completed; none was dropped.
- Aggregation was fixed beforehand: medians of runs for evaluation and
  throughput, percentiles pooled over all samples for propagation.
- The v0.1.30 numbers published on 2026-11-11 came from another session and
  are not used in any comparison here.

Host: Intel Core i5-8259U laptop, 8 GiB, macOS 15.8.1, Docker Desktop VM with
8 vCPU / 3.8 GiB, in desktop use with swap in use. Server limited to 1 vCPU /
512 MiB; PostgreSQL 16 and Redis 7 in containers.

## Before and after (all measured)

| Metric | v0.1.30 | v0.1.31 | Change |
|---|---|---|---|
| Node SDK `isEnabled()`, 50 % rollout | 7.16 µs | **1.00 µs** | 7.2× faster |
| Node SDK `isEnabled()`, boolean | 6.96 µs | 0.79 µs | 8.8× faster |
| Node SDK, other cases (rules, multivariate, prerequisites) | 7.1–8.7 µs | 2.2–4.0 µs | 1.9–4.0× faster |
| Node SDK p99 of batch means, 50 % rollout | 8.20 µs | 1.06 µs | |
| Rust core, all six cases | 80–441 ns | 84–456 ns | within 6 %; not attributed |
| Throughput at 1,000 clients (req/s, median, range) | 489 (398–502) | **591** (543–645) | +21 % |
| Throughput at 500 clients | 453 (444–496) | 547 (486–643) | +21 % |
| Throughput at 100 clients | 512 (426–534) | 546 (473–660) | +7 % |
| Throughput at 10 clients | 507 (412–529) | 485 (421–650) | inside both ranges |
| Server CPU per snapshot request, 1,000 clients | 1.92 ms | **1.51 ms** | −21 % |
| Server CPU per snapshot request, 10 clients | 1.84 ms | 1.84 ms | no difference |
| Server RSS peak at 1,000 clients | 60 MiB | 59 MiB | unchanged |
| Errors in any throughput run | 0 | 0 | |
| Propagation p50, 1,000 SDK clients | 37.3 ms | 39.2 ms | +5 % |
| Propagation p99, 1,000 clients, pooled over 6 runs | 75.4 ms | **198.3 ms** | higher (contended runs) |
| Propagation p99, 1,000 clients, median of the 6 run p99s | 75.5 ms | 84.1 ms | +11 % |
| Propagation p99, 1,000 clients, runs 3–6 | 74–78 ms | 73–86 ms | overlapping |
| Deliveries missed / reconnects (1,921,200 per version) | 0 / 0 | 0 / 0 | |
| Server RSS holding 1,000 streams | 43 MiB | 42 MiB | unchanged |
| Server CPU while propagating, 1,000 clients | 0.24 cores | 0.25 cores | unchanged |

## Reading it

- **The Node SDK is the release's large gain**, from reading only the
  attributes a flag references (PERFORMANCE-OPTIMIZATIONS.md O1). It matches the
  isolated A/B measured earlier in the milestone (8.8×, boolean).
- **Server CPU per snapshot is lower at 100–1,000 clients** (−11 % to −21 %), from
  decoding stored flags directly and reading flags and segments in one
  statement (O2, O3). At 10 clients this session shows no difference, while
  the isolated A/B measured −33 % there. Runs at 10 clients spread by
  28 % (v0.1.30) and 54 % (v0.1.31) at the same CPU use, more than the change.
  The −33 % figure stays the better estimate of the code change; this run
  neither confirms nor contradicts it at that level.
- **Propagation.** The p50s are equal. v0.1.31's first two propagation runs
  were slow at every client count, while the host was loaded from the image
  builds (15-minute load average 15.8 and 7.5); the PATCH response itself was
  slow in the same runs (p99 210 and 32 ms against 20–28 ms everywhere else),
  and the SDK processes used more CPU (1.28 and 1.05 cores against 0.82–0.91).
  Runs 3–6 overlap v0.1.30's at every client count. Two isolation runs (each
  SDK against the same scripted server; each server driven by the same SDK)
  showed no difference. RC2 changed nothing on the delivery path except the
  heartbeat, which does not fire on a stream carrying an update every 0.2 s, and
  0 streams were replaced by the new liveness deadline.
- **Why propagation has 6 runs.** After the first 2 slow runs, 3 more per
  version were added, alternated the same way, before anything was dropped or
  re-aggregated; nothing was. The decision was made after seeing results and
  is disclosed for that reason. The pooled p99 (198 ms) is the published
  headline, as the rule fixed in advance requires, with the median of the run
  p99s next to it.

**Verdict:** no measured regression attributable to the release. The
propagation tail is unresolved on this host. Follow-up: rerun propagation on a
dedicated Linux machine before quoting a p99 for v0.1.31 anywhere except next
to these caveats.

## README graphic

Generated from `docs/performance/results/v0.1.31/summary.json` by
`bench/public/scripts/render_svg.py`:

- `assets/benchmarks/performance-light.svg`, `assets/benchmarks/performance-dark.svg`
  ("Architecture & Performance": evaluation, throughput, CPU and memory,
  propagation, each with the v0.1.30 value from this session).
- `assets/benchmarks/cost-light.svg`, `assets/benchmarks/cost-dark.svg`
  ("Infrastructure Cost & Savings"; see COST-EFFICIENCY-BENCHMARKS.md).

`python3 bench/public/scripts/validate.py` (run in CI) re-renders both from the
summaries and fails if a committed SVG, a summary or the README's alt text is
out of date. See ARCHITECTURE-BENCHMARKS.md for what each tile shows.

## Not measured

Web, React Native and Flutter SDK evaluation; more than 1 vCPU; more than one
server instance; more than 1,000 SDK clients for propagation (raw connections
up to 10,000 are in the cost suite); a dedicated or Linux host.
