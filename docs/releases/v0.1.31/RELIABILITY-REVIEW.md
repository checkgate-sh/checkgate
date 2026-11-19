# Reliability review: v0.1.31

Scope: restarts, crashes, concurrent updates, network and storage
interruptions, SDK reconnects, stale and duplicated notifications, slow or
disconnected clients, and boundedness of every retry and buffer.

## Existing evidence carried forward

v0.1.30 added the multi-instance fault-injection suite
(`server/tests/reliability.rs`, 14 scenarios), the lifecycle suite
(`server/tests/lifecycle.rs`) and a daily extended run
(`.github/workflows/reliability.yml`). The extended run has passed every day
from 2026-11-12 to 2026-11-17 on `main`. The server-side sync path has not
changed since then, apart from R2 below, so those results still apply.

**Local re-run on 2026-11-18** (macOS, PostgreSQL 16 on :55432, Redis 7 on
:56379):

| Suite | Result |
|---|---|
| `reliability` | 13/14. `g2_client_that_never_reads_again_is_released` fails on macOS **as expected**: it tests `TCP_USER_TIMEOUT`, which exists only on Linux. It passes in Linux CI. |
| `lifecycle` | 8/8 |
| `integration`, `e2e_sse`, `rate_limit`, `setup_ownership`, `sdk_health`, `environment_isolation` | all pass |

Already verified and unchanged: lost Redis events are recovered by
reconciliation; dead subscriptions are detected by ping; graceful shutdown
closes streams within `SHUTDOWN_GRACE_PERIOD_SECS`; the SSE connection cap and
bootstrap semaphore hold; resyncs are spread with jitter; lagged receivers are
disconnected; scheduled jobs run exactly once across instances (`FOR UPDATE
SKIP LOCKED`); per-flag revisions follow commit order; SDKs drop events older
than the revision they have applied; reconnect replays are staged and applied
all at once; reconnect backoff in all four SDKs is capped and jittered; JS
impression and event buffers are capped at 10 000; every background task is
tied to the shutdown token; and the `RelayIndex` mutex is never held across
an `.await`.

## Findings

Each finding was traced to the code. Fixed findings have a regression test.

| ID | Finding | Severity | Status |
|---|---|---|---|
| R1 | **The Flutter SSE parser split each network chunk on `\n` with no carry-over.** A line spanning two socket reads lost its second half, so flags vanished from a large bootstrap. When the cut fell inside `event: ready`, `connect()` never completed and every later update stayed staged. | High | **Fixed.** `parseSseEvents` decodes bytes and re-splits them into whole lines with `LineSplitter`. Test: `SSE framing` cuts a frame set at every byte offset and also delivers it one byte at a time. Run against the same frames, the old algorithm produced wrong output at 116 of 149 cut points and lost `ready` at 17. |
| R2 | **A failed segment read after commit published the flag with its segment rules removed.** `load_env_segments(..).unwrap_or_default()` then expanded against an empty map. The event carried the new revision, so SDKs applied it and reconciliation treated it as relayed: the stripped flag stayed live until its next change. | High | **Fixed.** `flags::publish_flag_change` publishes nothing when segments cannot be read. It re-bootstraps this instance's clients of that environment, and other instances recover through reconciliation, as for a writer that dies after commit. Metric: `checkgate_resync_requests_total{trigger="segment_read_failed"}`. **RC2: now verified by fault injection** (`sse_faults::a_segment_read_failure_after_commit_publishes_nothing_stale_and_recovers`: the segments table is made unreadable right after the commit; with the v0.1.30 publish restored the test fails, the writer's client serving the new revision with `rules: []`). See SSE-FAULT-INJECTION.md. |
| R3 | **The Flutter poll fallback could roll back newer stream state.** A slow poll that landed after the stream reconnected re-applied old flags and reset revisions. Polls had no timeout and could pile up. | Medium | **Fixed.** A generation counter, a single in-flight request and a 30 s timeout, matching the JS SDKs. Test: `a poll response that lands after the stream took over is dropped`. |
| R5 | **The React Native SDK used a global `EventSource` that React Native does not provide**, and nothing told users to install one. Without a polyfill, `connect()` never settled. | Medium | **Fixed.** Without `EventSource` the SDK logs an actionable error and keeps flags current by polling; the install docs now require a polyfill. Test: `without a global EventSource › connect() falls back to polling and settles`. |
| R8 | **The poll snapshot silently dropped undecodable flags** (polling SDKs then delete them) and read flags and segments non-atomically. | Medium | **Fixed.** See CORRECTNESS-REVIEW.md C2. |
| R18 | **Argon2 ran inline on Tokio worker threads** in login, setup and user creation: about 19 MiB and tens of ms per call in release, seconds in debug. Concurrent logins (an unauthenticated endpoint) could occupy every async worker and stall all streams and requests on the instance. Found while investigating a `setup_ownership` failure under load (a concurrent-setup "loser" answered 500): the candidate failed 2 of 16 full-suite runs before this fix, against 0 of 8 for v0.1.30, and starvation of the task holding the advisory lock is the mechanism that fits. The logs of the failing runs were not kept, so this is a mechanism, not a proven root cause. | Medium | **Fixed.** Hashing and verification run on the blocking pool, at most one per CPU at a time. Test: `argon2_tests::hashing_does_not_block_the_async_runtime` (a ticker on a single-threaded runtime keeps running during a hash). After the fix, `setup_ownership` passed 10 of 10 full-suite runs. |
| D1 | **The all-in-one container SIGKILLed PostgreSQL and Redis on every stop.** The server was exec'd as PID 1, and once it exited the kernel killed the rest. Compose relied on Docker's 10 s stop timeout, below the server's 20 s grace period. | Medium (operational: crash recovery on every start; Redis AOF buffer lost) | **Fixed.** See DEPLOYMENT-READINESS.md and OPERATIONAL-FAILURE-TESTS.md. |

### Found and fixed in RC2

| ID | Finding | Severity | Status |
|---|---|---|---|
| R19 | **React Native never saw a stream failure.** `react-native-sse` (the required polyfill) supports `addEventListener('error')` but not `onerror`, and does not reconnect after a network error: one blip left the app on stale flags until restart. | High | **Fixed.** See SDK-CONNECTION-RELIABILITY.md. Before the fix the shared fault suite failed 19 of 21 scenarios on React Native. |
| R4 | No SDK detected a silently dead stream. | Medium | **Fixed in RC2**: named `heartbeat` event, `heartbeat_ms` on `connected`, connect and idle deadlines in all four SDKs. |
| R6 | 401/403 retried forever; `connect()` never settled. | Medium | **Fixed in RC2**: terminal `auth_failed`, `connect()` rejects, last flags kept, `connect()` retries. |
| R12 | Flutter `close()` + `connect()` could leave two SSE loops. | Low | **Fixed in RC2** (loop generation). |
| R13 | Web `disconnect()` during WASM init leaked a stream. | Low | **Fixed in RC2.** Also: one WASM engine per client (a fresh engine per `connect()` lost unchanged flags after a reconnect without storage). |
| F1 | The Flutter package was not importable as published and bundles no engine. | High (Flutter adopters) | **Import fixed** (`lib/`); engine bundling documented, not built. See SDK-END-TO-END-VALIDATION.md. |

### Open findings (not fixed in v0.1.31)

| ID | Finding | Severity | Disposition |
|---|---|---|---|
| R20 | **A delayed stream replay can briefly roll back flags polled meanwhile.** Found by the extended fault tier (seeds 173, 184). While an SDK is in poll fallback, a new stream's bootstrap replay is read by the server when the stream opens. If that stream's bytes arrive late (a slow or buffering link) and polls return newer state in between, `connected` stops polling and `ready` swaps in the older replay: flags go back to older revisions, and a flag deleted meanwhile can reappear. The live events queued behind the replay then restore the newest state (71 ms in seed 173; in general, as long as the slow stream takes to catch up). Poll snapshots carry no revisions, so the SDK cannot tell which is newer. **Pre-existing:** the v0.1.30 Node client fails seed 173 the same way. Affects all four SDKs (seed 173 fails on each; 184 on web only, depending on timing). Needs poll fallback active and a stream delayed by more than a poll interval. | Medium | **Open; not an RC2 regression.** Proposed fix (protocol addition, backward compatible: `Flag` ignores unknown fields): `/flags/snapshot` returns each flag's `revision`, and tombstones; SDKs record them when polling, and at `ready` keep, per flag, whichever of the replay and the polled state is newer. Server and all four SDKs; owner decision whether in v0.1.31 or next. Seeds 173 and 184 are recorded in `sdks/conformance/ci-seeds.json` as known failures (not in the CI set until fixed); the nightly extended tier keeps failing on them as a reminder. |
| R6b | Polling ignores `Retry-After`, and `pollIntervalMs` has no floor. (The 401 part of R6 is fixed.) | Low | Deferred. |
| R7 | Post-commit work (audit row, webhooks) runs inside the request future; a client disconnect can cancel it. The flag change itself is recovered by reconciliation. | Low–Medium | Deferred: needs a spawned task or an outbox. |
| R9 | Concurrent edits are last-write-wins with no `If-Match`. Merges never interleave (row lock), but two editors changing `rules` overwrite each other. | Low | Documented behaviour; optimistic concurrency is a feature. |
| R10 | Scheduled jobs retry permanent errors (404/409) every 60 s forever, so a job blocked by approval fires days late once approval is turned off. | Low | Deferred: needs a terminal-outcome state in the schema (a migration, which would break rollback; see UPGRADE-AND-ROLLBACK.md). |
| R11 | Webhooks: one untracked task per event, non-2xx not retried, ordering not guaranteed. Bodies are now capped (S3). | Low | Deferred. |
| R14 | Lag disconnects skip the resync spread, so all slow receivers reconnect at once after a segment edit touching more than 256 flags. | Low | Deferred. |
| R15 | After a disconnect, Node, web and RN keep serving the last flags; Flutter `close()` returns `false`. | Low | Behaviour difference documented in SDK-COMPATIBILITY.md. The doc drift (Flutter `disconnect()`, RN `connect()` returning void) is fixed in RC2. |
| R16 | `RelayIndex` entries for deleted flags and tombstones are pruned only by later activity. Both are bounded by the number of distinct keys. | Info | No action. |
| R17 | The Redis subscriber reconnects after a fixed 2 s with no jitter, so instances reconnect in step after a Redis restart. | Low | Deferred. |

## Retry and buffer inventory

| Loop / buffer | Bound |
|---|---|
| SDK stream reconnect (all four) | Exponential, capped at `maxReconnectDelayMs`, up to 30 % jitter; after 401/403: none (RC2) |
| SDK stream liveness (RC2) | One watchdog per client: connect deadline 30 s, idle deadline 3 × `heartbeat_ms` |
| SDK poll fallback | Fixed interval; JS SDKs single-flight with abort; Flutter single-flight with a 30 s timeout (new) |
| Server resync broadcast after a failed publish | One loop per instance, 1 → 30 s backoff, cancelled at shutdown |
| Redis subscriber reconnect | Fixed 2 s (R17) |
| Webhook delivery | 3 attempts (1, 5, 15 s); bodies capped at 1 KiB (new); 200 deliveries kept per webhook |
| Scheduled jobs | Every 60 s, unbounded on permanent errors (R10) |
| SSE per-connection fan-out | Broadcast capacity 256; lagging receivers are disconnected |
| SSE connections | `SSE_MAX_CONNECTIONS` (default 10 000) |
| Rate-limit buckets | Keyed by /64 prefix, with eviction |
| SDK impression/event buffers | 10 000 |
