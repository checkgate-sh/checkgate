# Performance optimizations: v0.1.31

Three changes were made; each came from a profile (CPU-AND-MEMORY-PROFILING.md)
and was measured afterwards (PERFORMANCE-COMPARISON.md). Each keeps results
identical. No `unsafe` code was added, and no dependency.

## O1: Node SDK reads only the attributes a flag references

**File:** `sdks/nodejs/src/lib.rs`

**Before.** `is_enabled(flag_key, user_key, user_attributes: HashMap<String, String>)`.
napi-rs converted the whole JS object on every call.

**After.** The binding takes the object (`Option<Object>`), looks up the flag
first, and reads only the attribute names its rules reference. It walks
prerequisites too, visiting each prerequisite flag once. Every other attribute
is never touched.

**Why results are unchanged.** The evaluator only ever looks up attributes by
the names in the rules it evaluates, so it receives exactly the entries it
would have found in the full map. Evidence: a differential run of the v0.1.30
and v0.1.31 addons over 300 generated flags × 400 generated contexts × 3 APIs
(**360 000 calls, 0 differing results**; `bench/ab/node-differential.cjs`,
`results/node-eval/differential.json`), and `sdks/nodejs/test/native-attributes.test.js`
(now run in CI after the addon build).

**Semantics, explicitly:**

| Input | v0.1.30 | v0.1.31 |
|---|---|---|
| Referenced attribute is a string | used | used |
| Referenced attribute is not a string (`{ plan: 7 }`) | throws `Failed to convert … into rust type String` | **same error** |
| Unreferenced attribute is not a string (`{ age: 30 }` with no `age` rule) | throws | ignored (in 57 345 of the 360 000 differential calls, v0.1.30 threw and v0.1.31 evaluated) |
| Attribute value `undefined` | absent | absent |
| `null`/`undefined` instead of the object | throws | no attributes |
| Inherited (prototype) property | not read (only own enumerable keys were listed) | not read (own properties only) |

**Gain:** 1.9× to 8.8× per call (boolean 6.9 → 0.78 µs). Flags with many
rules gain least, because they read more attributes.

**Rejected follow-up:** caching absent attribute names to skip repeated
`has_own_property` calls when several rules reference a missing attribute.
That needs a per-call `HashSet<String>` (an allocation) for a gain only on
flags with repeated missing attributes; not worth the complexity.

## O2: decode `jsonb` straight into `Flag` and segment rules

**Files:** `server/src/stream.rs` (SSE bootstrap and snapshot),
`server/src/api/segments.rs` (`load_env_segments`)

**Before:** `row.try_get::<serde_json::Value>("data")`, then `serde_json::from_value::<Flag>(v)`.
**After:** `row.try_get::<sqlx::types::Json<Flag>>("data")`, a single
`serde_json::from_slice` into the target type.

**Why results are unchanged.** It is the same `Deserialize` implementation
reading the same JSON text. The Criterion bench asserts both paths produce
equal `Flag`s for the fixture before timing. Decode failures behave as before:
the bootstrap and the snapshot fail (500), and an undecodable segment is
skipped.

**Gain:** 486 → 154 µs per 101 flags in isolation (3.2×), about 330 µs of CPU
saved per snapshot request and per SSE bootstrap of that size.

## O3: one statement for the snapshot

**File:** `server/src/stream.rs` (`read_snapshot`)

Segments and flags are read with one `UNION ALL` statement instead of two.
This was primarily a **correctness** fix (C2: both reads now come from one
snapshot). It also removes a database round trip per `/flags/snapshot`.

## Combined server effect

On the native snapshot A/B (concurrency 10): **server CPU per request
−33 %** (2 634 → 1 757 µs), throughput **+44 %** (442 → 635 req/s), p50
21.5 → 15.1 ms. RSS unchanged. In the published, CPU-bound 1 vCPU
configuration, −33 % CPU per request would allow roughly 1.5× the requests
per second. **This is a projection, not a measurement**; confirming it needs
the public suite run on the release commit.

## Considered and rejected

| Idea | Why not |
|---|---|
| Cache the serialized snapshot per environment | It would lift throughput much more, but needs invalidation tied to every flag and segment write, Redis-relayed changes and reconciliation on every instance. A stale cache would break the reliability guarantees, so it is a design change, not a v0.1.31 optimization. Recommended as a separate, designed change, with ETag / `If-None-Match` so polling SDKs can skip unchanged snapshots entirely. |
| Avoid `json!` → `Value` in `upsert_message` (SSE event encoding) | Runs once per change event, not per request; it does not appear in profiles. |
| Memoizing prerequisite results per evaluation | It would fix the fan-out (C1) exactly, but costs an allocation per evaluation of a flag with prerequisites. The 1 000-evaluation budget is O(1) and keeps every real configuration's semantics. |
| `unsafe` / SIMD JSON | Rejected by the milestone constraints; the gain does not justify it. |
| A web (WASM) attribute optimization like O1 | Not measured (no public WASM benchmark exists), so it is unjustified by evidence. The WASM binding now decodes attributes leniently (C3) through `serde_wasm_bindgen`, as before. |
