---
title: Infrastructure cost and efficiency
description: What it costs in CPU, memory and traffic to keep 100 to 10,000 clients connected to one Checkgate server, the same clients polling instead, and what that infrastructure would cost a month at dated cloud list prices.
---

# Infrastructure cost and efficiency

Understand the infrastructure required to run your own feature flags: what one
Checkgate server, under the same 1 vCPU / 512 MiB limit as the
[public benchmarks](./benchmarks.md), spends to keep clients up to date, and
how that compares with the same clients polling.

Four kinds of number appear below, and they are never mixed:

| Kind | Meaning |
|---|---|
| **Measured** | Read from the runs: CPU, memory, bytes, requests, freshness |
| **Calculated** | Measured per-event costs × a month of events, under stated assumptions |
| **Pricing** | Dated list prices, with sources (`bench/public/pricing.json`) |
| **Projected** | Calculated usage priced with those inputs. A projection, not a quote |

## Results

Measured 2026-11-18 (streaming) and 2026-11-19 UTC (polling) on the release
candidate's server image, one run per profile, every profile on a fresh stack.
Host and limits as in the [public benchmarks](./benchmarks.md#environment).
The first polling runs were replaced after a fix to how the harness measures
staleness (`raw/superseded-poll-staleness/README.md`); the streaming runs and
the server image are unchanged.

### What it shows

- **Streaming cost grows with connections, not with time.** Idle, one server
  held 10,000 streams at 14.7 % of one core and 448 MiB, sending each client a
  27-byte heartbeat every 15 s (6.5 KB an hour). Each flag change cost about
  250 bytes per client. A reconnect costs the full flag set again (26 KB here),
  and with the assumptions below reconnects and heartbeats are most of a
  streaming client's traffic.
- **10,000 streams is near this configuration's limit.** Peak RSS was 480 MiB of
  the 512 MiB limit, ramping up took 71 s with 16,063 attempts refused and
  retried (the bootstrap queue holds 5 at a time), a mass reconnect took 54 s
  at 91 % of the core, and a change took 439 ms (p50) to reach every client.
  Give a server holding that many streams more memory than 512 MiB.
- **Polling cost grows with clients ÷ interval**, and every poll here is a full
  15 KB snapshot and one PostgreSQL read. At 500 requests/s (5,000 clients
  every 10 s) PostgreSQL reached 94 % CPU (p90) and the server 76 %; 1,000
  requests/s (10,000 clients every 10 s) was **not sustained**: 65 % of requests
  failed and those that succeeded took 10 s.
- **Freshness.** Streaming delivered a change in 16–439 ms (p50, by client
  count). Polling's staleness is set by the interval: p50 half the interval,
  p99 about one interval (5 s / 10 s at 10 s, 30 s / 59 s at 60 s), at every
  client count that was sustained.
- **Projected cost.** The fixed infrastructure (server, PostgreSQL, Redis,
  storage) is $38.56 a month at these list prices, and one server at this size
  carried every streaming profile, so streaming costs the same from 100 to
  10,000 clients; its traffic stays under the 100 GB free egress allowance.
  Polling's projected cost is that same base plus egress for the snapshots,
  which is what makes it grow. Rows marked ¹ use more than the t4g.small's
  sustained CPU baseline (0.4 vCPU) and would draw down CPU credits or need a
  larger instance, which is not priced.

**Read polling's figures as an upper bound for polling in general.** Every
poll returns the whole flag set: Checkgate has no conditional requests
(`ETag` / `304 Not Modified`) on `/flags/snapshot` yet. A polling design that
answers unchanged snapshots with a 304 would send far less. And egress is
priced as internet egress: clients inside the same cloud network pay less or
nothing for it.

### Streaming (SSE), measured

| Clients | All ready after | Refused during ramp | Idle server CPU | Idle RSS / peak | Idle server traffic out | Heartbeats per client | Change delivered p50 / p99 | Missed | Server CPU during changes | Mass reconnect: all ready again | Server CPU during reconnect | PostgreSQL CPU p90 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 100 | 0.6 s | 0 | 0.15 % | 16 / 30 MiB | 735 B/s | 6.5 KB/h | 16 ms / 44 ms | 0 | 0.425 % | 4.0 s | 7.7 % | 11.2 % |
| 1,000 | 3.2 s | 0 | 1.5 % | 47 / 55 MiB | 6.7 KB/s | 6.5 KB/h | 33 ms / 72 ms | 0 | 1.64 % | 6.3 s | 46.5 % | 11.6 % |
| 5,000 | 19.9 s | 1,129 | 6.89 % | 206 / 246 MiB | 33.1 KB/s | 6.5 KB/h | 203 ms / 703 ms | 0 | 17.5 % | 22.8 s | 72 % | 65 % |
| 10,000 | 70.6 s | 16,063 | 14.7 % | 448 / 480 MiB | 66.1 KB/s | 6.5 KB/h | 439 ms / 989 ms | 0 | 39.7 % | 53.9 s | 90.7 % | 78.5 % |

Per client, at every count: bootstrap 26.3 KB (101 flags), 248 B per flag change, 26.3 KB per reconnect.

### Polling `GET /flags/snapshot`, measured

| Clients | Every | Target req/s | Achieved | Error rate | Server CPU | Server RSS | Request p50 / p99 | Bytes per request | Staleness p50 / p99 | PostgreSQL CPU p90 | Sustained |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 100 | 60 s | 1.667 | 1.667 | 0.0 % | 0.678 % | 12 MiB | 6 ms / 9 ms | 15.3 KB | 30.0 s / 59.4 s | 11.7 % | yes |
| 100 | 30 s | 3.333 | 3.333 | 0.0 % | 1.07 % | 12 MiB | 6 ms / 9 ms | 15.3 KB | 15.0 s / 29.7 s | 12.5 % | yes |
| 100 | 10 s | 10 | 9.999 | 0.0 % | 2.7 % | 12 MiB | 5 ms / 8 ms | 15.3 KB | 5.0 s / 9.9 s | 14.3 % | yes |
| 1,000 | 60 s | 16.667 | 16.666 | 0.0 % | 4.25 % | 13 MiB | 5 ms / 9 ms | 15.3 KB | 30.0 s / 59.4 s | 15.7 % | yes |
| 1,000 | 30 s | 33.333 | 33.33 | 0.0 % | 8.2 % | 13 MiB | 5 ms / 9 ms | 15.3 KB | 15.0 s / 29.7 s | 19.8 % | yes |
| 1,000 | 10 s | 100 | 100.007 | 0.0 % | 20.2 % | 13 MiB | 5 ms / 9 ms | 15.3 KB | 5.0 s / 9.9 s | 33.5 % | yes |
| 5,000 | 60 s | 83.333 | 83.332 | 0.0 % | 17.3 % | 13 MiB | 5 ms / 8 ms | 15.3 KB | 30.0 s / 59.4 s | 29.5 % | yes |
| 5,000 | 30 s | 166.667 | 166.645 | 0.0 % | 30.7 % | 13 MiB | 4 ms / 11 ms | 15.3 KB | 15.0 s / 29.7 s | 44.2 % | yes |
| 5,000 | 10 s | 500 | 499.995 | 0.0 % | 76.3 % | 21 MiB | 7 ms / 277 ms | 15.3 KB | 5.0 s / 9.9 s | 93.5 % | yes |
| 10,000 | 60 s | 166.667 | 166.663 | 0.0 % | 30 % | 13 MiB | 4 ms / 9 ms | 15.3 KB | 30.0 s / 59.4 s | 43.6 % | yes |
| 10,000 | 30 s | 333.333 | 333.319 | 0.0 % | 54.8 % | 19 MiB | 5 ms / 77 ms | 15.3 KB | 15.0 s / 29.7 s | 75.5 % | yes |
| 10,000 | 10 s | 1000 | 999.864 | 64.9 % | 95.5 % | 424 MiB | 10.0 s / 10.1 s | 4.7 KB | 12.9 s / 57.9 s | 93.5 % | **no** |

### Monthly traffic, calculated

| Clients | SSE | Poll 60 s | Poll 30 s | Poll 10 s |
|---|---|---|---|---|
| 100 | 0.9 GB · 0.01 M streams opened | 66.9 GB · 4.38 M requests | 133.9 GB · 8.76 M requests | 401.6 GB · 26.28 M requests |
| 1,000 | 8.7 GB · 0.12 M streams opened | 669.3 GB · 43.80 M requests | 1,339.1 GB · 87.61 M requests | 4,015.3 GB · 262.83 M requests |
| 5,000 | 43.4 GB · 0.61 M streams opened | 3,346.9 GB · 219.02 M requests | 6,693.3 GB · 438.05 M requests | 20,077.6 GB · 1,314.14 M requests |
| 10,000 | 86.9 GB · 1.22 M streams opened | 6,693.5 GB · 438.05 M requests | 13,382.5 GB · 876.10 M requests | not sustained |

SSE per client per month: heartbeats 4.7 MB, flag changes 753.4 KB, reconnects 3.2 MB; total 8.7 MB.

### Monthly cost, projected

| Clients | SSE | Poll 60 s | Poll 30 s | Poll 10 s |
|---|---|---|---|---|
| 100 | $38.56 | $38.56 | $41.61 (egress $3.05) | $65.71 (egress $27.14) |
| 1,000 | $38.56 | $89.80 (egress $51.24) | $150.08 (egress $111.52) | $390.94 (egress $352.37) |
| 5,000 | $38.56 | $330.78 (egress $292.22) | $631.96 (egress $593.39) | $1,836.55 (egress $1,797.99) ¹ |
| 10,000 | $38.56 | $631.98 (egress $593.42) | $1,233.99 (egress $1,195.43) ¹ | not priced (not sustained) |

Fixed lines in every priced row: server $12.26, server disk $0.64, PostgreSQL $11.68, PostgreSQL storage $2.30, Redis $11.68 (= $38.56); egress varies.

¹ Server CPU above the t4g.small's 0.4 vCPU sustained baseline (polling at the
target rate; streaming: idle CPU, as changes and reconnects are short bursts).
The comparison uses CPU measured on this x86_64 host; t4g is arm64.


## Method

`bench/public/run.sh cost` runs every profile on a **fresh stack** (new
database, new containers) with the published limits: server 1 vCPU (CFS quota)
/ 512 MiB, PostgreSQL 2 vCPU / 1 GiB, Redis 0.5 vCPU / 256 MiB, generators on
their own vCPUs. The flag set is the public suite's: 101 flags, a 15 KB
snapshot. Clients are raw protocol clients (`bench/public/sdk/connections.mjs`),
not SDKs: what is measured is the server and the wire, and raw clients let one
generator hold 10,000 connections. They authenticate with the SDK key and back
off on refusal as the SDKs do (3 s × 2ⁿ, at most 30 s, up to 30 % jitter).

**Streaming (SSE)**, per client count:

1. **Ramp**: open every stream, 50 at a time per worker, until each has
   received `ready`.
2. **Idle, 60 s**: no flag change. Server CPU and RSS, server network bytes,
   and heartbeat bytes received by the clients.
3. **Updates**: 30 changes to one flag, one every 2 s. Bytes per change per
   client, and time from the change request to the event at each client.
4. **Mass reconnect**: every stream dropped at once; each client reconnects
   after 3.0–3.9 s, as an SDK's first retry does. Time until all are ready
   again, and bytes re-sent.

**Polling**, per client count and interval (10, 30, 60 s): every client
requests `GET /flags/snapshot` every interval (start times spread over one
interval). After one interval of warm-up, a window of at least 3 intervals
with a flag change every 2 s. Requests per second achieved against the target
(clients ÷ interval), errors, server CPU, bytes per request, and **staleness**:
for each change, how long until each client first saw it.

Server CPU and RSS come from the server's `/proc` entry and its network bytes
from its network namespace (`/proc/<pid>/net/dev`, all of `eth0`: SDK traffic
plus PostgreSQL and Redis traffic). PostgreSQL and Redis CPU and memory come
from `docker stats`. Bytes counted at the clients are application bytes (no
TCP or TLS overhead); the benchmark runs without TLS.

**Sustained** means: every client connected, every change reached every
client, and every client reconnected (SSE); at least 95 % of the target
request rate with no errors (polling). Profiles that were not sustained are
reported as such and get no price.

## Assumptions behind the monthly figures

Traffic that depends on how an application uses flags, not on Checkgate:

| Assumption | Value | Effect |
|---|---|---|
| Flag changes a day | 100 | each sends one event (about 250 bytes) to every streaming client |
| Stream reconnects per client a day | 4 | each re-sends the full flag set (about 26 KB with 101 flags) |
| Days a month | 30.42 | |

Change them in `bench/public/pricing.json` and rerun
`bench/public/run.sh summarize`; the measured per-event costs do not change.
With these values, reconnects dominate streaming traffic, so a deployment that
restarts clients often should expect proportionally more.

## Prices

AWS us-east-1, on-demand, Linux, single-AZ, accessed **2026-11-19** from
third-party price aggregators (AWS's own pricing pages could not be read
programmatically). Check the AWS pricing pages for your region before
budgeting.

| Item | Price | Sources |
|---|---|---|
| Server: EC2 t4g.small (2 vCPU burstable, 2 GiB) | $0.0168 / h | [cloudquell](https://cloud.cloudquell.com/aws/ec2/t4g.small/), [cloud-credits](https://cloud-credits.org/prices/ec2/t4g-small), [cloudprice](https://cloudprice.net/aws/ec2/instances/t4g.small) |
| Server disk: EBS gp3, 8 GB | $0.08 / GB-month | [AWS EBS pricing](https://aws.amazon.com/ebs/pricing), [factualminds](https://www.factualminds.com/blog/amazon-ebs-pricing-orphaned-volumes-snapshots/) |
| PostgreSQL: RDS db.t4g.micro (2 vCPU burstable, 1 GiB) | $0.016 / h | [codewithsql](https://codewithsql.com/cloud-databases/aws/rds/pricing), [selfhost.dev](https://selfhost.dev/blog/aws-rds-cost-breakdown-2026/), [bytebase](https://www.bytebase.com/dbcost/rds/instance/db.t4g.micro/) |
| PostgreSQL storage: gp3, 20 GB (the RDS minimum) | $0.115 / GB-month | [bminfotrade](https://bminfotrade.com/blog/cloud-computing/aws-rds-pricing-2026-mysql-postgresql-and-sql-server-costs), [codewithsql](https://codewithsql.com/cloud-databases/aws/rds/pricing) |
| Redis: ElastiCache cache.t4g.micro (0.5 GiB) | $0.016 / h | [holori](https://calculator.holori.com/aws/elasticache/cache.t4g.micro), [vantage](https://instances.vantage.sh/aws/elasticache/cache.t4g.micro) |
| Internet egress | first 100 GB a month free, then $0.09 / GB | [egresscost](https://egresscost.com/aws/data-transfer-pricing), [eon](https://www.eon.io/blog/aws-data-transfer-pricing) |

Not included: TLS termination or a load balancer, backups beyond the free
allowance, Multi-AZ, support plans, taxes, and reserved or spot discounts.

The t4g.small is burstable: 0.4 vCPU in total is its sustained baseline, and
the measured server CPU is compared with that. db.t4g.micro has the same 1 GiB
of memory as the benchmark's PostgreSQL container, and cache.t4g.micro more
than the benchmark's Redis used.

## What this is and is not

- Polling here is **Checkgate's own snapshot endpoint, polled**, on the same
  server, database and flag set. It shows what the architecture choice costs,
  not how another product performs.
- Streaming is not free: every connection is held open, receives a heartbeat
  every 15 s when nothing changes, and gets the whole flag set again on every
  reconnect. Those costs are measured and included above.
- Evaluation itself costs no request in either model: SDKs evaluate locally.
  A service that evaluates each flag remotely would add a request per decision;
  that architecture is not measured here.
- One server, one machine (see [Environment](./benchmarks.md#environment)).
  Client counts the server did not sustain under the limit are reported as not
  sustained, not extrapolated.

## Reproduce

```bash
bench/public/run.sh cost            # every profile, about 50 minutes
COST_CLIENTS=1000 COST_POLL_INTERVALS=30 bench/public/run.sh cost   # one pair
bench/public/run.sh summarize       # cost-summary.json and the README graphic
python3 bench/public/scripts/validate.py
```

Raw results: `docs/performance/results/v0.1.31/raw/cost-*.json`; aggregated:
`docs/performance/results/v0.1.31/cost-summary.json`.
