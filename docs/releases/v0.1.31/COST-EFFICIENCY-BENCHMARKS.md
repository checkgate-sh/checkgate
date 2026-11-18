# README "Infrastructure Cost & Savings": method, results and limits

The README's second benchmark graphic, `assets/benchmarks/cost-{light,dark}.svg`,
and its full write-up, [docs/performance/cost-efficiency.md](../../performance/cost-efficiency.md).
This page records how it was produced for v0.1.31 and what it may and may not
be used to claim.

## Four kinds of number, never mixed

| Kind | What | Where it comes from |
|---|---|---|
| Measured | Server CPU, RSS and network bytes; PostgreSQL and Redis CPU and memory; bytes per heartbeat, change, bootstrap and poll; delivery time and polling staleness | `bench/public/run.sh cost` (`bench/public/sdk/connections.mjs`), one fresh stack per profile, server limited to 1 vCPU / 512 MiB |
| Calculated | Monthly traffic and requests | measured per-event bytes × the stated assumptions (100 flag changes a day, 4 stream reconnects per client a day, 30.42 days) |
| Pricing | Hourly and per-GB list prices | `bench/public/pricing.json`: AWS us-east-1 on-demand, accessed 2026-11-19, each with its sources (third-party aggregators; AWS's own pages could not be read programmatically) |
| Projected | Monthly cost | calculated usage × pricing, **only for profiles that were sustained** |

No figure is extrapolated beyond what ran: 10,000 clients polling every 10 s
was not sustained by one server, so it has no traffic figure and no price.

## Workloads

- Streaming (SSE): 100, 1,000, 5,000 and 10,000 connected clients; ramp, 60 s
  idle, 30 changes, mass reconnect.
- Polling `GET /flags/snapshot`: the same client counts every 10, 30 and 60 s,
  with a change every 2 s to measure staleness.
- Raw protocol clients, not SDKs (one generator holds 10,000 connections);
  they back off on refusal as the SDKs do.

## Headline results (1,000 clients, the graphic's middle panel)

| | Streaming | Polling every 60 s | every 30 s | every 10 s |
|---|---|---|---|---|
| Freshness p50 (measured) | 33 ms | 30 s | 15 s | 5 s |
| Server CPU (measured) | 1.5 % idle | 4.3 % | 8.2 % | 20 % |
| Traffic a month (calculated) | 8.7 GB | 669 GB | 1,339 GB | 4,015 GB |
| Cost a month (projected) | $38.56 | $89.80 | $150.08 | $390.94 |

Streaming at every client count measured: $38.56 a month, the fixed
infrastructure (EC2 t4g.small, RDS db.t4g.micro with 20 GB, ElastiCache
cache.t4g.micro, 8 GB EBS); traffic 0.9–87 GB, inside the free 100 GB of
egress. All tables: docs/performance/cost-efficiency.md.

## Resource findings

- **Memory per stream**: about 46 KB (448 MiB idle at 10,000 streams over a
  base of about 12 MiB; 480 MiB peak, of a 512 MiB limit). `SSE_MAX_CONNECTIONS` defaults to 10,000, so a
  server sized at 512 MiB is at its edge there; recommend at least 1 GiB for
  that many streams. To be added to the self-hosting sizing notes.
- **Ramp-up at 10,000 streams** took 71 s with 16,063 refusals, by design: the
  bootstrap queue admits 5 at a time and clients back off.
- **Every poll is a PostgreSQL read** (one consistent statement). Polling at
  500 requests/s took PostgreSQL to 94 % CPU (p90) on 2 vCPU; 1,000 requests/s
  was not sustained (65 % errors).
- **Polling bytes are the full snapshot** (15.3 KB with 101 flags) every time:
  there is no `ETag` / `304` yet (recommended in PERFORMANCE-OPTIMIZATIONS.md).

## What it must not be used to claim

- A saving against another product: polling here is Checkgate's own endpoint
  on the same infrastructure.
- A general saving of streaming over polling: polling with conditional
  requests, or inside a private network where egress is free, costs much
  less than shown; the projection is an upper bound for polling.
- "No network overhead": streaming has a measured cost (heartbeats, a full
  flag set per reconnect), included above.
- A quote: list prices change, and taxes, support, Multi-AZ, backups beyond
  the free allowance, TLS termination and load balancers are not included.
  CPU was measured on x86_64; t4g is arm64.

## Corrections during the run

The first polling runs measured staleness wrongly (each poll counted only the
change it returned, so changes replaced before the client polled were never
counted, and p50 came out 1.2–1.4 s at every interval). The harness was fixed
and every polling profile was run again on the same server image; the first
runs are kept, with the reason, in
`docs/performance/results/v0.1.31/raw/superseded-poll-staleness/`. The fixed
harness was layered on the candidate SDK image (`candidate-r2`: the same image
with the new `connections.mjs`); the server and load-generator images are
byte-identical to those of the first run.
