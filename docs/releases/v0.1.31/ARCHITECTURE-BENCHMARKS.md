# README "Architecture & Performance": what it shows and where each number comes from

The README's first benchmark graphic, `assets/benchmarks/performance-{light,dark}.svg`.
Everything in it is **measured** on v0.1.31, and each comparison is with v0.1.30
**measured in the same session** (PUBLIC-BENCHMARK-RESULTS.md). Nothing is
calculated, projected or taken from another environment.

## Tiles

| Tile | Value | Exact source in `docs/performance/results/v0.1.31/summary.json` | Selection rule (fixed before the runs) |
|---|---|---|---|
| SDK evaluation | 1.0 µs, v0.1.30 7.2 µs | `headline.evaluation` ← `evaluation.node_sdk.cases.percentage_rollout.median_ns` | The 50 % rollout case through the Node SDK's public `isEnabled()`; median of 3 runs of 100 batch means. The context line gives the p95/p99 **of batch means** (one call is too short to time alone) and the Rust core alone (303 ns). |
| Server throughput | 591 req/s, v0.1.30 489 | `headline.throughput` ← `throughput.levels[concurrency=1000].rps_median` | The level with the highest median among levels with zero errors; v0.1.30 at the same level. The tile states the concurrency and that level's p99 latency (2.4 s: a closed loop at saturation queues). |
| CPU and memory | 1.51 ms CPU per request, v0.1.30 1.92 ms; peak RSS 59 MiB | `headline.efficiency` ← `1e6 / throughput.levels[concurrency=1000].requests_per_server_cpu_second` | The same level as the throughput tile. |
| Update propagation | p99 198 ms, v0.1.30 75 ms; p50 39.2 ms; 0 missed; median run p99 84 ms | `headline.propagation` ← `propagation.profiles[clients=1000].visible_from_request_ms.p99_ms` | p99 at the largest client count that completed in every run, pooled over all 6 runs. The median of the run p99s is shown beside it because two contended runs dominate the pooled tail (see below). |

The strip below the tiles states the architecture the numbers measure, without
numbers of its own:

- **Local evaluation**: SDKs evaluate in-process; no request per decision.
- **Server-sent events**: one persistent connection per client carries changes.
- **Rust server**: measured inside a 1 vCPU / 512 MiB container.

The footer gives the host, Docker VM, limits, run counts (3; propagation 6),
the commit, "Measured under documented workloads and hardware. Results may vary
by deployment.", and the methodology link.

## The propagation tile

The pooled p99 is higher for v0.1.31 (198 ms against 75 ms) because v0.1.31's
first two propagation runs were slow at every client count while the host was
loaded, with the write path itself slow in the same runs. Runs 3–6 overlap
v0.1.30's. The graphic shows the change as measured ("2.63× worse") rather than
hiding it, and gives the median run p99 (84 ms) in the context line. The full
analysis and the follow-up (a rerun on a dedicated Linux host) are in
PUBLIC-BENCHMARK-RESULTS.md and docs/performance/benchmarks.md.

## How it is built and kept honest

- `bench/public/run.sh summarize` → `summarize.py` (raw runs → `summary.json`)
  → `render_svg.py` (→ the two SVGs). Deterministic: the same summary renders
  the same bytes.
- `python3 bench/public/scripts/validate.py`, in CI (`benchmarks` job, "Benchmark results and graphic"), fails
  when: the summary is not what `summarize.py` makes from the committed raw
  runs; an SVG differs from a fresh render; an SVG has script, an external
  reference, a web font or no `<title>`/`<desc>`, or exceeds 40 kB; the README's
  "Architecture & Performance" section does not state each headline number
  (alt text); a link in that section or the benchmark docs does not resolve; a
  documented `run.sh` command does not exist.
- The SVG carries a `<desc>` with every number and its scenario for screen
  readers, and the README repeats the headline numbers in the image's alt text.
- Light and dark variants are selected with `<picture>` and
  `prefers-color-scheme`.

## What it does not claim

- No comparison with another product.
- No capacity beyond what ran: one server, 1 vCPU, up to 1,000 SDK clients for
  propagation.
- No web, React Native or Flutter evaluation figures (not benchmarked).
- The v0.1.30 values are from this session, not the 2026-11-11 publication.
