# CPU and memory profiling: v0.1.31

The rule for this phase was to profile before optimizing. Each change in
PERFORMANCE-OPTIMIZATIONS.md started from a measurement described here.

## Tools

| Tool | Used for |
|---|---|
| macOS `/usr/bin/sample` (statistical stack sampler, 1 ms interval, no root needed for own processes) | Server CPU hot paths under snapshot load (8 s per build) |
| Criterion (`bench/benches/*.rs`) | Isolating one cost (core evaluation, snapshot decoding) |
| `bench/public/sdk/eval.mjs` | Node SDK per-call latency via the public API |
| Controlled variation (calls with 0 vs 5 attributes, flags with and without rules) | Attributing Node SDK time to attribute conversion versus evaluation |
| `ps -o cputime,rss` around a load window | Server CPU per request and resident memory |

`perf`, `heaptrack` and `samply` were not available on the macOS host;
`sample` covers the CPU questions asked here. Allocation counts per evaluation
come from the existing counting allocator in `bench/benches/evaluation.rs`.

## Finding P1: Node SDK time is attribute conversion, not evaluation

| Observation | Value |
|---|---|
| Rust core, boolean flag | 79 ns |
| Node `isEnabled`, boolean flag, 5 attributes | 6 857 ns |
| Marginal cost per attribute (v0.1.30 public benchmark analysis) | ≈1.3 µs |

The binding declared `user_attributes: HashMap<String, String>`. napi-rs
converts that **eagerly** on every call: it enumerates the object's property
names, then for each one makes one property read and two UTF-8 conversions,
allocating two `String`s. That runs before the flag is even looked up. A
boolean flag with no rules paid the full cost for attributes it never reads,
and so did a missing flag. This is about 85 % of the call.

## Finding P2: snapshot CPU goes to JSON trees and the allocator

Top-of-stack samples, v0.1.30 server under `/flags/snapshot` load (101 flags),
excluding idle waits (`__psynch_cvwait`, `kevent`). The raw excerpt is in
`results/profile/top-of-stack-v0.1.30.txt`.

| Rank | Frame (demangled) | What it is |
|---|---|---|
| 1–5 | `memmove`, `free_tiny`, `tiny_free_no_lock`, `tiny_malloc_should_clear`, `tiny_free_list_add_ptr` | Allocator churn |
| 6 | `core::str::from_utf8` | UTF-8 validation of row text |
| 7 | `serde_json::ser::format_escaped_str` | Writing the response |
| 8 | `serde_json::Value::deserialize` (from slice) | **jsonb text → `Value` tree** |
| 9 | `BytesMut::put_slice` | Response buffer |
| 10 | `serde_json::read::skip_to_escape` | Parsing |
| 11 | `Map<String, Value>::deserialize_any` for `Flag` | **`Value` → `Flag`** |
| 12 | `BTreeMap<String, Value>::insert` | Building the intermediate maps |

The `Value` path builds a `BTreeMap` per JSON object (each flag, rule and
variant), then walks it a second time to build the `Flag`, then frees it. The
allocator frames at the top are largely that churn.

Isolated with Criterion (`bench/benches/snapshot_decode.rs`, the 101-flag
public fixture):

| Decode path | Time per 101 flags |
|---|---|
| text → `Value` → `Flag` (v0.1.30) | 486 µs [469, 507] |
| text → `Flag` | 154 µs [153, 154] |

## Finding P3: one extra round trip per snapshot

v0.1.30 read segments and flags in two statements. Natively (DB through Docker
Desktop port forwarding) the snapshot path is latency-bound: p50 21.5 ms at
concurrency 10, while the server used only about 1.1 cores. Combining the two
reads into one statement (needed anyway for correctness, C2) removes a round
trip.

## Memory

| Observation | v0.1.30 | v0.1.31 |
|---|---|---|
| Server RSS after a 25 s snapshot load window (native) | 10.0–10.1 MiB | 9.7–10.2 MiB |
| Server RSS in the all-in-one container after boot (Linux, new `process_resident_memory_bytes`) | — | 13.3 MiB |
| Allocations per core evaluation | 0 (1 for multivariate) | unchanged (core hot path untouched; the prerequisite budget is a stack `u32`) |

No growth in RSS was observed in any run. The v0.1.30 propagation benchmark
already showed RSS held flat (12–15 MiB) across 600 updates with up to 1 000
clients; the sync path is unchanged.

### Bounded growth: memory added by the hardening

| Change | Memory effect |
|---|---|
| Configuration limits (S1) | **Removes** an unbounded amplification: an expanded flag is bounded by 50 segment references × the 64 KiB body cap, where before it could reach roughly 180 MB from one write |
| Webhook response cap (S3) | **Removes** an unbounded `text()` read; at most 1 KiB per delivery |
| Argon2 permit (R18) | At most one 19 MiB Argon2 working set per CPU at a time (before: one per concurrent login, unbounded) |

## Not profiled

- The **SSE fan-out** path (`stream.rs` loop): the v0.1.30 propagation
  benchmark shows the server at 0.04–0.05 cores while delivering to 1 000
  clients, so it is not a bottleneck.
- **Lock contention**: the server's only shared mutable structures on hot
  paths are the `DashMap`s (connected clients, SDK flag store) and the sync
  `RelayIndex` mutex, which is never held across an `.await`. The samples show
  no meaningful time in `__ulock_wait2` (79 samples of ~33 000).
