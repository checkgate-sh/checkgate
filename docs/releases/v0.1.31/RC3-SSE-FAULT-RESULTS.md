# SSE fault results: v0.1.31 RC3

The RC2 fault suites (SSE-FAULT-INJECTION.md), extended for R20 and rerun on
the RC3 tree. Same invariants (I1–I7), same timing, same step counts: no
assertion was weakened and no seed or step removed.

## What RC3 added to the suites

| Addition | Where | Purpose |
|---|---|---|
| `holdNextStream(ms)` fault: the replay is read when the stream opens (as the server does) and its bytes are held | `fault-server.mjs` | Lets polls overtake a replay on purpose (the R20 precondition) |
| `waitHeld` step | `scenario.mjs` | Writes happen only after the held replay was read, so the sequence is deterministic |
| `delayOn` with an explicit per-write delay | `scenario.mjs` | Makes the window between `ready` and the next live event observable (60 ms); random seeds still use 30 ms, so they mean the same steps as in RC2 |
| `/flags/snapshot?v=2` (revisions, tombstones) and `legacySnapshot` (the pre-0.1.31 array) | `fault-server.mjs` | Same wire format as the real server, and a legacy-server mode |
| Named: *a delayed replay does not roll back newer polled flags (R20, seed 173)* | `scenario.mjs` | Deterministic form of the seed 173 sequence |
| Named: *a delayed replay does not resurrect a flag deleted meanwhile (R20, seed 184)* | `scenario.mjs` | Deterministic form of the seed 184 sequence |
| Named: *poll fallback against a pre-0.1.31 server (no snapshot revisions) converges* | `scenario.mjs` | Compatibility: new SDK, old snapshot format |
| Seeds 173 and 184 moved from `known_failures` into the CI set | `ci-seeds.json` | Mandatory on every pull request |
| `trace-r20.mjs` | `sdks/conformance` | Prints the client-side timeline of a seed or named scenario, against any build of the Node SDK |
| `SimSdk` follows the RC3 contract (`?v=2` polls, per-flag merge, staged stale check) | `server/tests/common/sdk.rs` | The server-level randomized suite checks the same rule against real servers; `divergence` now also checks polled revisions |
| Live checks: concurrent updates; delayed replay after a newer poll; server SIGKILL and restart | `live.mjs`, `live-restart.mjs` | Phase E against a real server and every SDK's real transport |

## Deterministic R20 regressions: before and after

Run on the same host, the same harness; "before" is a detached worktree of
`dev` 8f97832 (RC2 merged) with the RC3 harness copied in.

| Scenario | Node (native) | Web | React Native | Flutter (Rust) | v0.1.30 Node |
|---|---|---|---|---|---|
| R20, seed 173 form: before | fail | fail | fail | fail | fail |
| R20, seed 184 form: before | fail | fail | fail | fail | fail |
| R20, seed 173 form: RC3 | pass | pass | pass | pass | — |
| R20, seed 184 form: RC3 | pass | pass | pass | pass | — |
| Random seed 173: before | fail | fail | fail | fail | fail |
| Random seed 184: before | pass | pass | pass | fail | — |
| Random seeds 173 and 184: RC3 | pass | pass | pass | pass | — |

Failures before the fix are all I2: for example `I2 alpha: went back from rev
4 to 1` (rollback) and `I2 alpha: resurrected at rev 1 after delete rev 3`
(resurrection).

## CI tier (every pull request)

13 named scenarios + 14 seeds (1–12, 173, 184) per SDK, RC3 tree, macOS host:

| SDK | Result |
|---|---|
| Node (native engine) | 27/27 |
| Web (stand-in engine) | 27/27 |
| React Native (stand-in engine) | 27/27 |
| Flutter (Rust engine via FFI) | 27/27 |

## Extended tier

Seeds 1–200 per SDK (40 steps each: 9,802 random steps per SDK, 66 key
revocations and recoveries) plus the 13 named scenarios, RC3 tree, macOS
(Intel) host, one SDK at a time, 2026-11-19. The same seeds and steps as the
RC2 extended run.

| SDK | Scenarios passed | Failed | Recovery after healing, p50 / p95 / max | Silent stream replaced after (named) | Max streams at once | RC2 (same seeds) |
|---|---|---|---|---|---|---|
| Node (native engine) | **213 / 213** | none | 11 / 309 / 670 ms | 327 ms | 1 | 199/200 seeds (173: R20) |
| Web (stand-in engine) | **213 / 213** | none | 11 / 294 / 430 ms | 336 ms | 1 | 198/200 (173, 184: R20) |
| React Native (stand-in engine) | **213 / 213** | none | 11 / 313 / 438 ms | 332 ms | 1 | 199/200 (173: R20) |
| Flutter (Rust engine via FFI) | **213 / 213** | none | 13 / 311 / 446 ms | 300 ms | 1 | 199/200 (173: R20) |

No violation of any invariant on any SDK: no rollback or resurrection (I2),
no partial or corrupt state (I1), no flag vanishing without a delete (I3), no
flapping at an unchanged revision (I4), never more than one stream (I5), every
`connect()` settled (I6), every client converged after healing (I7, bound
3.06 s). Recovery times are in line with RC2 (p50 11–14 ms, max 419–654 ms
there): the merge adds no measurable delay.

## Live, against a real server

`scripts/sdk-live-e2e.sh all` (debug server, PostgreSQL 16, Redis 7, 1 s
heartbeat), RC3 tree, 2026-11-19: **44/44** (10 checks × 4 SDKs + a server
SIGKILL and restart per SDK). Raw: `results/rc3/live-e2e.jsonl`.

| Check (new in RC3 in bold) | Node | Web | React Native | Flutter |
|---|---|---|---|---|
| Bootstrap, updates, delete, reconnect, half-open, poll fallback, invalid key, revoked key, shutdown (RC2's 8) | pass | pass | pass | pass |
| **Concurrent updates arrive in order and converge** (4 flags × 8 sequential writes concurrently + a 6-write burst on one flag) | pass, converged 1 ms after the last write | pass, 1 ms | pass, 0 ms | pass, 0 ms |
| **A delayed replay never rolls back a newer poll (R20)**: replay read, held 2 s and trickled; a change and a delete delivered by polls first | pass; replay applied 2.2 s later, nothing went back | pass (2.7 s) | pass (3.1 s) | pass (3.6 s) |
| **Server SIGKILL, 2 s down, restart, two writes** | pass; newest version 359 ms after restart | pass (438 ms) | pass (436 ms) | pass (53 ms) |

Negative control: the same suite with the RC2 Node client (`CONFORMANCE_NODE_ENTRY`
pointing to `dev` 8f97832) fails exactly one check, *a delayed replay never
rolls back a newer poll (R20)*: "state went back to the delayed replay". Every
other check passes.

## Server level

`server/tests/sse_faults.rs` with the RC3 `SimSdk` (real servers, PostgreSQL
16, Redis 7): 5/5, including randomized seeds 1–3 with writes through two
instances, link faults, Redis outages and instance restarts.

**Extended: seeds 1–40 × 30 steps: pass** (533 s, 2026-11-19). Every seed
converged after healing within the test's bound, with no revision regression
and no flapping, now also with the polled revisions checked against the
database. Recovery after healing over the seeds whose output was kept (17–40;
the log of 1–16 was cut by the capture filter): p50 333 ms, p95 2.42 s, max
2.82 s (RC2, seeds 1–40: p50 271 ms, p95 1.97 s, max 2.92 s; the timings are
dominated by the clients' reconnect backoff and Redis/instance faults).

## Required assertions

| Assertion | Result |
|---|---|
| Zero stale configuration rollbacks | **Met.** 0 I2 violations in 852 SDK scenarios (213 × 4) and 40 server seeds; the deterministic R20 scenarios pass on every SDK; the live R20 check passes on every SDK (and fails on the RC2 Node client) |
| Zero unexpected flag flapping | **Met.** 0 I4 violations (evaluation sampled every 25 ms per flag and user) |
| Zero indefinite stale states after recovery | **Met.** 0 I7 violations: every scenario converged within 3.06 s of healing (max recovery 670 ms); server seeds converged within their bound |
| Zero partial configuration application | **Met.** 0 I1 violations; named *replay cut mid-frame* and *event cut mid-frame* pass; malformed `?v=2` bodies are rejected whole (unit tests, every SDK) |
| Zero unbounded reconnect attempts | **Met.** Reconnects stay on the capped, jittered backoff; the revoked-key scenarios (66 per SDK) make 0 requests after `auth_failed`; the merge adds no request |
| Zero leaked subscriptions | **Met.** Max 1 stream at once on every SDK in every scenario; 0 streams after close (I5); live *shutdown releases every stream* passes |
| Eventual convergence within documented bounds | **Met.** I7 bound 3.06 s (test timing) in every scenario; live recoveries 102–3 110 ms with a 1 s heartbeat (8 s bound) |

## Limits

Unchanged from RC2: the SDK level runs on a host (React Native's transport is
the real polyfill over an XHR like React Native's; the web transport is
`eventsource` with browser error semantics); web and React Native use the
stand-in engine; faults are injected on loopback.
