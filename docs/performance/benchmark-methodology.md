---
title: Benchmark methodology
description: How Checkgate's evaluation core and control plane are benchmarked, and how to reproduce the published baseline.
---

# Benchmark methodology

Two harnesses, measuring different things. Neither number should be read as
the other.

| Harness | Measures | Excludes |
|---|---|---|
| `bench/benches/evaluation.rs` (Criterion) | The Rust evaluation core shared by every SDK: `FlagStore` lookup + evaluation, in-process | SDK binding overhead (N-API, wasm-bindgen, JSI, Dart FFI), JSON at the SDK boundary, network |
| `server/examples/loadgen.rs` | The control plane: SSE ramp-up, held connections, change propagation, snapshot throughput, REST latency, server CPU/RSS | SDK-side processing (one event apply is measured by the core bench instead) |

## Evaluation core (Criterion)

```bash
cargo bench -p checkgate-bench --bench evaluation          # full run (~5 min)
cargo bench -p checkgate-bench --bench evaluation -- --quick
```

Workloads, all with a representative user (stable key + five attributes):

| Group | Cases |
|---|---|
| `boolean_lookup_eval/{10,100,1000,10000}` | Boolean flag in a store of N flags |
| `missing_flag/{N}` | Lookup of an absent key |
| `evaluation_1000_flags/targeting_1_rule`, `targeting_10_rules` | Rules that do not match, then one that does (worst case: every rule is checked) |
| `evaluation_1000_flags/percentage_rollout` | 50 % rollout (hashing) |
| `evaluation_1000_flags/weighted_multivariate` | 60/30/10 variant split via `evaluate_variant` |
| `evaluation_1000_flags/prerequisite_chain/{2,3,5}` | Flag requiring a chain of prerequisites |
| `concurrent_evaluation_1000_flags/readers/{1,2,4,ncpu}` | Aggregate throughput, readers on separate threads |
| `concurrent_evaluation_1000_flags/readers_with_writer/{1,4}` | Same, with one thread replacing flags continuously |
| `sdk_apply_upsert_event` | Parse one SSE change event and replace the flag (what an SDK does per event) |

Concurrent cases report wall time per evaluation summed over all threads
(lower is more aggregate throughput); each thread cycles through all 1 000
keys from a precomputed list.

Before the groups run, the bench prints an allocation profile measured with a
counting global allocator: allocations per evaluation for each case, retained
bytes per stored flag at each store size, and retained-memory growth after
replacing 1 000 flags 20 times.

Criterion's own outputs (`target/criterion/**`, HTML report) hold every sample;
the time range printed is the 95 % confidence interval of the mean.

## Control plane (`loadgen`)

```bash
cargo build --release -p server --bin server --example loadgen
target/release/examples/loadgen \
    --server-bin target/release/server \
    --database-url postgres://postgres:postgres@localhost:5432/postgres \
    --redis-url redis://localhost:6379 \
    --connections 100,1000,5000 --updates 200 --out results.json
```

Options: `--instances` (2), `--flags` (100), `--snapshot-secs` (10),
`--snapshot-concurrency` (32), `--db-max-connections` (10). The role in
`--database-url` needs `CREATEDB`; a database `checkgate_loadgen` is
re-created. Raise `ulimit -n` for large profiles (each connection uses a
descriptor in the generator and one in a server).

What it does, in order:

1. Starts `--instances` release servers sharing one database and one Redis
   (rate limits off, `SSE_MAX_CONNECTIONS=0`, `RUST_LOG=warn`, metrics on a
   private port), completes setup, and creates 100 flags (a quarter each:
   boolean, 25 % rollout, one targeting rule, two-way multivariate) plus the
   flag it toggles.
2. **REST latency** — 200 sequential `GET …/flags?limit=100` and 200 `PATCH`es
   from a session-authenticated client.
3. **Snapshot throughput** — `--snapshot-concurrency` clients loop on
   `GET /flags/snapshot` against both instances for `--snapshot-secs`.
4. For each connection profile:
   - **Ramp-up** — opens N SSE streams round-robin across instances with at
     most 200 handshakes in flight (a reconnect wave), timing each from
     request to its `ready` event.
   - **Held** — waits 5 s idle, then reads each server's RSS and CPU time from
     `/proc`.
   - **Propagation** — 200 `PATCH`es of one flag through instance 0, one every
     50 ms; every stream records when each change event arrived.
5. Scrapes each instance's `/metrics` for server-side histogram means.

### Propagation latency without clock skew

Sends and receives are timestamped on the generator's monotonic clock, in one
process, so no clock synchronization is involved. Reported per profile:

| Field | Meaning |
|---|---|
| `write_to_visible` | From sending the `PATCH` to the event arriving on a stream — one sample per (update, stream). The full user-visible path: HTTP, authentication, the PostgreSQL write and commit, audit log, publish, Redis fan-out to every instance, SSE delivery. |
| `visible_after_http_response` | How long after the `PATCH` response the event arrived (0 when it arrived first — the server publishes before responding). Approximates the fan-out and SSE delivery share. |
| `fanout_spread_first_to_last` | Per update, from the first stream to receive it to the last — the cost of delivering one event to N connections. |
| `missed_deliveries` | (update, stream) pairs that never arrived. Must be 0. |

Server-side components come from the servers' own histograms:

| Component | Source |
|---|---|
| Write handling (DB statements, commit, audit, publish) | `checkgate_http_request_duration_seconds{method="PATCH"}` mean |
| Redis publish | `checkgate_flag_publish_duration_seconds` mean |
| DB + audit share | the difference of the two |
| SDK application of one event | `sdk_apply_upsert_event` in the core bench |

### Caveats

- The generator and the servers share the machine and compete for CPU. Pin
  them apart (`taskset -c 0-3 …server`, `taskset -c 4-5 …loadgen`) for
  steadier numbers; the published baseline did not.
- CPU is read in `USER_HZ` (100 on mainstream Linux).
- PostgreSQL and Redis run in local containers: network latency between
  services is loopback. Add your real RTT for remote databases.
- Results vary run to run; publish more than one repetition and the raw JSON.
