# Operational failure tests: v0.1.31

v0.1.30 built the fault-injection infrastructure (`server/tests/common/chaos.rs`,
`cluster.rs`, `sdk.rs`): multi-instance clusters, process kills, Redis and
network faults, a simulated SDK. It runs on every PR and daily in an extended
configuration. This milestone reused it rather than building new soak tooling.

## Executed in this milestone

| Failure | How | Result |
|---|---|---|
| Unexpected process termination, writer killed between commit and publish, commit with no event | `tests/reliability.rs` F1–F3 | pass (local, 2026-11-18) |
| Redis loss, silent Redis subscription, publish failure | `tests/reliability.rs` | pass |
| Repeated restarts, rolling restarts across instances | `tests/reliability.rs`, `tests/lifecycle.rs` | pass |
| Slow or stuck reader (never reads again) | `reliability::g2` | **macOS: fails as designed** (needs Linux `TCP_USER_TIMEOUT`); passes in Linux CI |
| Saturated SSE capacity, bootstrap queue full, draining | `tests/lifecycle.rs` | pass (8/8) |
| Container stop with the bundled database (D1) | `docker stop` on v0.1.30 and v0.1.31 images, then PostgreSQL's own log | v0.1.30: crash recovery on every start; **v0.1.31: clean shutdown** |
| Killed container (`docker kill`) leaving a stale `postmaster.pid` | `scripts/docker-smoke.sh` | pass |
| Unreachable external database at start | `scripts/docker-smoke.sh` | refused with a clear error |
| Database read failure on the publish path (R2) | Code path reviewed; a fault at exactly that statement is not injectable with the current harness | **not executed**; behaviour verified by review and the new metric label |
| Undecodable stored flag | `tests/hardening.rs` (row corrupted with SQL) | snapshot fails loudly (500) instead of silently dropping the flag |
| Login stampede blocking the runtime (R18) | `argon2_tests::hashing_does_not_block_the_async_runtime` | pass |
| Mixed-version cluster (rolling upgrade) | `bench/ab/upgrade-rollback.sh` | pass: changes cross v0.1.30 ↔ v0.1.31 in both directions |
| SDK: chunk-split SSE frames (Flutter, R1) | Test cutting the frames at every byte offset and delivering them one byte at a time | pass. The old parser was wrong at 116 of 149 cut points |
| SDK: stale poll after the stream reconnects (Flutter, R3) | Unit test with a delayed `MockClient` | pass |
| SDK: no `EventSource` (React Native, R5) | Unit test | falls back to polling; `connect()` settles |

## Not re-run, with reason

| Test | Reason |
|---|---|
| Extended reliability soak (`reliability.yml`) | Runs daily on `main`; green from 2026-11-12 to 2026-11-17. The sync path changes in this release are confined to the R2 error branch. It will run on the release commit after merge. |
| 75-minute public benchmark (sustained load) | The v0.1.30 run held RSS flat under sustained snapshot and propagation load; recommended on the release commit to refresh the README graphic. |
| Android native build verification | No change in the React Native native layer beyond the shared attribute decoder; CI job `sdk-native-react-native` covers it. |

## Gaps for future work

1. **Inject a fault at a specific statement**, to cover R2 and similar
   post-commit paths. For example, a test-only hook in the DB layer, or a
   proxy that fails the Nth query.
2. **SDK idle watchdog (R4)**, then a test that blackholes the stream without
   closing it. This is the most important open reliability gap for clients.
3. **Run real SDKs in the reliability suite.** It uses a Rust simulated SDK,
   so the JS and Dart reconnect code is tested only by unit tests.
