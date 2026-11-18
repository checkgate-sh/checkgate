# SSE fault injection: v0.1.31 RC2

Randomized, reproducible tests of the SSE reconnect path (prompted by a
community suggestion: drop the stream at random during updates), at two
levels:

- **SDK level** (`sdks/conformance`): every SDK's real client and transport
  against a scriptable fault server that speaks the SDK protocol.
- **Server level** (`server/tests/sse_faults.rs`): real server processes,
  PostgreSQL and Redis, with faults on the client links, the Redis links,
  the processes and PostgreSQL itself.

## SDK level

### Faults

The fault server (`fault-server.mjs`) can, between any two bytes: close every
stream (dropped link, restart); stall them with the socket left open (half-open
connection, lost heartbeats); cut an event mid-frame; cut the bootstrap replay
mid-frame; deliver an event twice; replay an older revision right after a
reconnect (reordering across the reconnect boundary); fragment writes to one
byte each; delay them; refuse connections; answer 503; delay snapshot responses
(a poll in flight while the stream reconnects); and reject the key with 401.
It holds the full revision history, so the suite knows exactly which revision
of a flag a client serves.

### Scenarios

- **Named** (10, the same for every SDK): half-open stream detected; quiet
  healthy stream not replaced (new server); quiet stream from a pre-0.1.31
  server not replaced; replay cut mid-frame; event cut mid-frame; older event
  after reconnect; duplicate events; restart during an update burst; slow poll
  versus the stream; revoked key and recovery.
- **Random**: 40 steps per seed (writes, deletes, bursts of 5 rapid updates to
  one flag, the faults above, waits) and, for one seed in four, a key
  revocation and recovery at the end. The generator is mulberry32: a seed is
  the same step list in every SDK and on every machine.

### Invariants (sampled every 25 ms)

| | |
|---|---|
| I1 | every flag version served existed on the server (no partial or corrupt state) |
| I2 | per flag, the revision never goes back; a deleted flag returns only at a newer revision |
| I3 | a flag disappears only after the server deleted it |
| I4 | at an unchanged revision, the evaluation for the same user is unchanged (no flapping from reconnects) |
| I5 | at most 2 streams at once, exactly 1 after recovery, 0 after close |
| I6 | `connect()` settles |
| I7 | after healing, the client serves exactly the server's flags within `CONVERGE_MS` (3.06 s at test timing) |

Named scenarios add: silent stream replaced within 3 heartbeats + 400 ms; no
reconnect of a quiet healthy stream; `auth_failed` with zero further requests,
then recovery on `connect()`.

Timing is production timing scaled down: heartbeat 100 ms (idle deadline
300 ms), backoff 20–200 ms, connect deadline 500 ms, poll every 100 ms after 3
failures.

### Results: CI tier (named + `ci-seeds.json`, seeds 1–12)

On the release candidate tree, macOS host:

| SDK (transport, engine) | Scenarios | Pass | Silent stream replaced after | Recovery after healing (median / max) | Max streams at once |
|---|---|---|---|---|---|
| Node (`eventsource`, native addon) | 22 | 22 | 323 ms | 0 / 333 ms | 1 |
| Web (`eventsource` as a browser, stand-in engine) | 22 | 22 | 331 ms | 0 / 335 ms | 1 |
| React Native (`react-native-sse`, stand-in engine) | 22 | 22 | 322 ms | 0 / 330 ms | 1 |
| Flutter (Dart client, Rust engine via FFI) | 22 | 22 | 285 ms | 1 / 294 ms | 1 |

Node also passed with the stand-in engine. A recovery of 0 means the client was
already current when the faults were healed.

### Results: extended tier

Seeds 1–200 per SDK (40 steps each, about 9,750 steps per SDK, 65 key
revocations), plus the 10 named scenarios, on the release candidate tree, macOS
host, one SDK at a time, 2026-11-19:

| SDK | Seeds passed | Failed seeds | Recovery after healing, p50 / p95 / max | Silent stream replaced after (named) | Max streams at once |
|---|---|---|---|---|---|
| Node (native engine) | 199 / 200 | 173 | 11 / 297 / 419 ms | 323 ms | 1 |
| Web (stand-in engine) | 198 / 200 | 173, 184 | 12 / 308 / 624 ms | 343 ms | 1 |
| React Native (stand-in engine) | 199 / 200 | 173 | 11 / 277 / 654 ms | 326 ms | 1 |
| Flutter (Rust engine via FFI) | 199 / 200 | 173 | 14 / 308 / 470 ms | 296 ms | 1 |

Every named scenario passed on every SDK. Every failure is the same finding,
**R20** (RELIABILITY-REVIEW.md), and every violation is invariant I2 (a flag
going back to an older revision; in seed 184 also a deleted flag reappearing).
No failure of I1, I3–I7: nothing corrupt, no flapping at an unchanged
revision, no extra streams, every `connect()` settled, every client converged
after healing.

**R20, as traced in seed 173 (Node, instrumented):** `/stream` fails, so the
client polls; a restart and a 30 ms write delay give the next stream a replay
that the server read when the stream opened (alpha at revision 23) but that
reaches the client late, while polls meanwhile return revision 26. `connected`
stops the polling, and `ready` swaps in the older replay. The live events
queued behind the replay bring the client back to revision 26 71 ms later.
Poll snapshots carry no revisions, so no SDK can tell which state is newer.
The v0.1.30 Node client fails seed 173 the same way: pre-existing, not caused by
RC2. Seed 184 (web only; passes on a second run) has the same preconditions:
503s on `/stream` (poll fallback), then a restart with delayed writes.

The seeds are recorded in `ci-seeds.json` under `known_failures`, not in the
CI set, until R20 is fixed.

### What the suite caught

| Found | Where | Outcome |
|---|---|---|
| Half-open stream never detected; stale flags indefinitely (seeds 2, 11, 12) | v0.1.30 Node client | R4; fixed. The same scenarios pass on RC2 |
| No error ever reached the SDK; 0 open streams after any interruption, 19 of 21 scenarios failed | previous React Native client | R19; fixed |
| `_markReady` did not settle a `connect()` issued after an auth failure on a client that had been ready | RC2 work in progress | fixed before commit |
| A stream that trickles bytes without completing a frame kept Flutter attached | RC2 Flutter client, **seed 11** | liveness now counts complete frames; seed kept in `ci-seeds.json` as a regression |
| Flags-then-evaluate read across two process round trips raced a delete | the harness (Flutter adapter) | atomic `sample` command |
| A delayed stream replay rolls back flags obtained by polling meanwhile (seeds 173, 184) | every SDK, and the v0.1.30 Node client | **R20**, open: needs per-flag revisions in `/flags/snapshot` |

Reproduce a seed: `CONFORMANCE_SEEDS=<seed> node --test sdks/conformance/flutter.mjs`
(or the SDK's own runner; see `sdks/conformance/README.md`). A failure writes the
seed, the violations and the full step trace to `$CONFORMANCE_TRACE_DIR`.

## Server level

`server/tests/sse_faults.rs`, PostgreSQL 16 and Redis 7 in containers,
`SSE_HEARTBEAT_INTERVAL_SECS=1`, clients are `SimSdk` (the SDKs' rules,
including the new deadlines and terminal 401/403).

| Test | Result |
|---|---|
| A quiet stream gets heartbeats (≥ 10 in 15 s) and is never replaced (1 attempt, 0 deadline trips) | pass |
| A stranded client stream (no FIN, no bytes, ever) is replaced and the change made meanwhile arrives, 3 times in a row, each within 6 s | pass |
| A revoked key is terminal: one 401, no further attempt or poll in 3 s, last flags kept; a restored key works again | pass |
| Randomized faults, seeds 1–3 (CI), 25 steps | pass; converged 168–381 ms after healing |
| **Randomized faults, seeds 1–40 (extended), 30 steps** | **pass**; converged after healing p50 271 ms, p95 1.97 s, max 2.92 s; no revision regression, no flapping |
| C1: segment read failure after commit (below) | pass |

Randomized steps: writes and rollout changes through either of two instances;
deletes; client link sever, strand, refuse and blackhole; Redis outages per
instance; instance kill and restart. Three clients: two through fault links,
one direct. The generator is SplitMix64 written out in the test, so a seed
means the same steps across dependency upgrades. Reproduce with
`CHECKGATE_FAULT_SEEDS=<seed> cargo test -p server --test sse_faults randomized`.

### C1: segment read failure (R2), now executed

1. Segment `staff` (email ends with `@acme.test`); flag `beta` referencing it.
   Both clients (one per instance) hold `beta` with the segment expanded.
2. The fault: `ALTER TABLE segments RENAME TO segments_unreadable`. Nothing on
   the flag write path reads segments, so the next write commits; the
   post-commit expansion (`publish_flag_change`) is the first read to fail.
3. `PATCH beta {rollout_percentage: 50}` → 200; the revision advances.
4. For 4 s, every 20 ms: neither client ever holds `beta` without its segment
   rule, and staff keep access. `GET /flags/snapshot` answers 500 instead of
   stripped flags. The writer logged that it skipped the publish because the
   segments were unreadable (and counted
   `checkgate_resync_requests_total{trigger="segment_read_failed"}`). The other
   instance's reconciliation found the unrelayed revision and resynced its
   clients; their bootstraps failed and they kept the previous snapshot.
5. Table restored: both clients converge to the new revision with the segment
   expanded in **446 ms**, and the snapshot serves it.

**Reproduces the original bug:** with the v0.1.30 publish put back (expand
against an empty map, publish anyway), the test fails at step 4: the writer's
client serves revision 2 with `rules: []`.

## Tiers

| Tier | What | Where |
|---|---|---|
| CI (every PR) | SDK: 10 named + 12 seeds per SDK; server: the five tests above, seeds 1–3 | `ci.yml` jobs `sdk-node`, `sdk-web-rn`, `sdk-flutter`, `server-integration` |
| Extended (nightly, on demand) | SDK: seeds 1–300 per SDK (input `seeds`); server: the reliability suites repeated, and 20 new randomized seeds × 30 steps per round | `reliability.yml` jobs `sdk-faults-extended`, `fault-injection-soak` |

## Limits

- The SDK level runs on a host: React Native's transport is the real polyfill
  over an XHR implementation like React Native's, not React Native itself; the
  web transport is `eventsource` with browser error semantics, not a browser.
- The web and React Native runs use a deterministic stand-in engine (the WASM
  build is stubbed in their unit-test jobs; JSI exists only on a device). Node
  and Flutter run the real engine. Engine correctness itself is tested in
  `core/` and by the live suites.
- Network faults are injected at the HTTP/TCP level on loopback; no packet loss
  or latency distribution of a real network.
