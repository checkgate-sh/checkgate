# Correctness review: v0.1.31

Scope: deterministic flag evaluation, rollouts, rule precedence, environment
isolation, defaults, concurrency, invalid context, and consistency across SDKs.

## Architecture facts that shape the review

- **There is only one evaluator.** `core/src/evaluator.rs` is compiled into
  every SDK: Node (N-API), web and edge (WASM), React Native (JSI → C FFI) and
  Flutter (Dart FFI). No SDK reimplements rule matching or hashing; the JS and
  Dart code handles transport and caching only. Cross-SDK consistency of
  *decisions* therefore holds by construction. What can differ is how each
  binding **decodes the input context** (see C3).
- **The server does not evaluate flags.** It stores them, expands segments and
  distributes them (`/stream`, `/flags/snapshot`). Evaluation correctness is a
  property of `core`; distribution correctness is covered in the reliability
  review.
- `integrations/checkgate-go` is an admin-API client with no evaluator, and the
  CLI generates typed accessors with no evaluator. Neither is affected.

## Guarantees and evidence

| Guarantee | Status | Evidence |
|---|---|---|
| Same flag + same user key + same attributes → same decision, every time and in every store instance | **VERIFIED** | `rollout_is_deterministic_and_monotonic_in_percentage`: 2 000 users × 101 percentages, each evaluated repeatedly, against two stores, with and without untargeted attributes. |
| A user in a rollout stays in it while the percentage only grows (raising never removes, lowering never adds) | **VERIFIED** (new) | The same exhaustive test. This follows from `bucket = murmur3("{flag}:{user}") % 100` being fixed per user and compared with `<`. |
| Buckets do not change between releases | **VERIFIED** (new) | `rollout_buckets_are_pinned_across_releases` pins 7 (flag, user) pairs, including empty keys and multi-byte UTF-8, at each bucket's threshold. `core/src/hashing.rs` is byte-identical to v0.1.30. `hashing::tests` check the streaming hash against the reference `murmur3` crate. |
| Share of users enabled ≈ percentage | **VERIFIED** | `rollout_share_tracks_the_percentage` (1/10/25/50/90 % over 20 000 users) and the public benchmark's 4 898/10 000 at 50 %. The modulo bias is 96 in 2³², which is negligible. |
| Weighted variants: sticky per user, follow the weights, and independent of the rollout gate | VERIFIED (existing) | `test_weighted_variants_*`, `large_equal_weights_distribute_across_both_variants`. |
| Precedence: disabled → prerequisites → rules (first match wins, bypassing rollout) → rollout → variants/default | VERIFIED (existing) | `test_first_matching_rule_wins`, `test_weighted_variants_bypassed_by_matching_rule`, `test_prerequisite_*`. Documented in `docs/guide/concepts.md`. The evaluation diagram in `docs/guide/architecture.md` left out prerequisites; it is corrected in this release. |
| Defaults: a missing flag → `false`/`null`; disabled → `disabled_value` or `false`; a non-boolean enabled flag without a default → `null` | VERIFIED (existing) | `test_disabled_flag_returns_disabled_value_or_false`, `test_enabled_boolean_default_is_true_nonbool_is_null`. |
| Missing attribute: satisfies `not_equals`, fails every other operator | VERIFIED (existing, documented) | `test_not_equals_missing_attribute_matches`. This is intended and documented, but it is why C3 failed open. |
| Prerequisite cycles and depth terminate and fail closed | VERIFIED (existing) | `test_prerequisite_cycle_fails_closed_without_hanging`. |
| Prerequisite **fan-out** terminates in bounded time | **CONFIRMED ISSUE → FIXED** (C1) | `test_prerequisite_fan_out_is_bounded_and_fails_closed`, `test_prerequisite_diamonds_within_budget_still_evaluate`. |
| Evaluation during concurrent updates sees one complete flag version | **VERIFIED** (new) | `concurrent_updates_never_produce_a_mixed_result`: 4 readers and 1 writer flip between two versions 20 000 times. Every result equals version A's or version B's answer. |
| Environment isolation of identical flag keys on every distribution path | VERIFIED (existing, CI) | `server/tests/environment_isolation.rs` covers REST, snapshot, SSE bootstrap, live deltas across instances, delete/recreate and restart. It passed locally on 2026-11-18. |
| Invalid context: a non-string attribute value | **CONFIRMED ISSUE → FIXED** (C3) | See below. |

## Confirmed issues

### C1: prerequisite fan-out was exponential (fixed)

Evaluation recursed into every prerequisite, bounded only by depth (10).
`all()` short-circuits on failure, so cycles were cheap. When every
prerequisite is *satisfied*, though, each level multiplies the work: a ladder
of 10 flags that each list the next one 8 times costs 8⁹ ≈ 1.3 × 10⁸ leaf
evaluations. Before the fix, the regression test was still running when the
120 s timeout killed it.

**Fix.** Each top-level evaluation carries a budget of 1 000 prerequisite
evaluations. Once it is spent, the remaining prerequisites are unsatisfied
(fail closed, like the depth guard). For every configuration within the budget
the semantics are unchanged, including the short-circuit order. Real
configurations use a handful of prerequisites; doubling at every level over 9
levels (510 evaluations) still evaluates normally. The server also rejects
duplicates, self-references and more than 50 prerequisites on write
(`server/src/limits.rs`).

### C2: `/flags/snapshot` disagreed with the SSE bootstrap (fixed)

- It read segments and flags in **two statements** outside a transaction, so a
  segment edit committed between them paired flags with segments from a
  different moment. The SSE bootstrap uses one REPEATABLE READ transaction.
- It **silently dropped** any flag row that failed to decode
  (`filter_map(..ok())`), while the bootstrap fails. A polling SDK treats an
  absent key as deleted, so one such row turned a flag off for every polling
  client. A realistic trigger is a newer server writing a new operator during
  a rolling upgrade.

**Fix.** One `UNION ALL` statement reads both, so both come from a single
snapshot even at READ COMMITTED; it is also one round trip instead of two. An
undecodable flag now fails the request (500), as in the bootstrap. Test:
`tests/hardening.rs::snapshot_expands_segments_and_fails_on_an_undecodable_flag`.

### C3: invalid attribute values erased the whole context in three SDKs (fixed)

The web (`serde_wasm_bindgen::from_value`), React Native and Flutter
(`serde_json::from_str`) bindings decoded attributes into
`HashMap<String, String>` and fell back to an **empty map** on any error. One
number (`{ plan: "free", age: 30 }`) removed `plan` as well, and the user then
passed `plan not_equals free`. Node threw a conversion error for the same input.

**Fix.** `checkgate_core::attributes::string_attributes` keeps every string
value and skips each non-string value on its own. All three bindings use it.
The skipped attribute still counts as missing (documented semantics), but it
no longer takes the others with it.

| Input `{ plan: "free", age: 30 }` vs rule `plan not_equals free` | v0.1.30 | v0.1.31 |
|---|---|---|
| Web / edge (WASM) | **enabled** (all attributes dropped) | disabled |
| React Native, Flutter | **enabled** (all attributes dropped) | disabled |
| Node | throws a conversion error | disabled (`age` is not referenced, so it is not read) |

Node now reads only the attributes the flag's rules reference (see
PERFORMANCE-OPTIMIZATIONS.md). A *referenced* attribute that is not a string
still throws, as before. A differential run of 360 000 calls comparing the
v0.1.30 and v0.1.31 Node addons found **0 differing results**: wherever v0.1.30
returned a value, v0.1.31 returned the same one.

## Remaining gaps

- **Property tests use exhaustive deterministic loops, not a property-testing
  library.** For a 0–100 percentage domain this is stronger than random
  sampling, and it adds no dependency. A fuzz target for flag JSON decoding
  is still recommended.
- **Pinned buckets are checked on the host target only.** The WASM build
  (32-bit `usize`) is not run in CI. The hash works on `u32` and byte slices,
  so it is platform-independent by construction, but no test runs it under
  wasm32.
- Mixed prerequisite snapshots: one evaluation reads prerequisites from the
  live store, so an update landing during an evaluation can pair a new
  prerequisite with the old parent flag. SDKs apply a reconnect snapshot as a
  whole before notifying listeners, but single live deltas apply one by one.
  This is inherent to per-flag updates and documented here; it is not a
  defect.
