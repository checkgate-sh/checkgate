# Release readiness: v0.1.31 RC3

| | |
|---|---|
| Candidate | `fix/v0.1.31-rc3-consistency`, from `dev` 8f97832 (RC2, PR #42, merged) |
| Date | 2026-11-19 |
| Previous decision | CONDITIONAL GO (RC2-RELEASE-READINESS.md): R20 open, owner to decide |
| Owner decision taken | Fix R20 before releasing v0.1.31 |
| Decision | **GO** (§4) |

## 1. What RC3 changed

| Change | Where | Evidence |
|---|---|---|
| **R20 fixed.** `GET /flags/snapshot?v=2` adds each flag's revision and the environment's tombstones (from the existing `flag_revision_seq` and `flag_tombstones`; no migration). Without `v` the endpoint runs the exact statement and returns the exact body of earlier releases | `server/src/stream.rs` | Unit tests; `hardening::revisioned_snapshot_carries_revisions_and_tombstones` (revisions equal the database's, tombstones after a delete, re-revision with a segment edit, re-create outranks tombstone, every `v` variant) |
| Every SDK (Node, web, React Native, Flutter) polls with `?v=2` and merges a complete set (replay on `ready`, or a poll) per flag with what it learned **after requesting it**: the newer version wins; everything learned before is replaced, as before. A local counter, not a clock, orders requests and knowledge | `sdks/*/index.js`, `checkgate_flutter.dart` | Unit tests (6 new per JS SDK, 6 in Flutter); fault suites; live suite |
| Duplicate events and deletes of flags not held no longer notify `onChange`; staged replay events are revision-checked | same | Unit tests |
| Fault harness: `holdNextStream`, `waitHeld`, revisioned and legacy snapshots, two deterministic R20 scenarios, one legacy-server scenario; seeds 173 and 184 in the CI set | `sdks/conformance` | RC3-SSE-FAULT-RESULTS.md |
| Server tests' `SimSdk` follows the new contract; `divergence` checks polled revisions | `server/tests/common` | `sse_faults` 5/5 and seeds 1–40 |
| Live suite: concurrent updates; delayed replay after a newer poll; server SIGKILL and restart per SDK | `live.mjs`, `live-restart.mjs`, `sdk-live-e2e.sh` | 44/44 |
| A/B tooling: `snapshot-ab.sh` `QUERY`/`CAND_QUERY`, `throughput --query`, `node-apply-ab.cjs` | `bench/ab`, `server/examples` | RC3-PERFORMANCE-REGRESSION.md |
| Docs: API reference and architecture (`?v=2`), CHANGELOG (fix, upgrade note, known limitations), R20 marked fixed in the RC2 reports. **Two dead links that failed the docs build** (`docs/performance/benchmarks.md` → excluded `releases/`; the site deploys from `main`) now point to GitHub | `docs/` | `npm run build` in `docs/` passes (failed on `dev`) |

No migration, no new dependency, no change to evaluation, SSE events or
authentication.

## 2. Evidence

All on this macOS (Intel) host unless marked CI.

| Category | Requirement | Result |
|---|---|---|
| R20 | Reproduced and fixed | Reproduced on the unfixed tree (seed 173 on all four SDKs, 184 on Flutter) and traced (RC3-R20-ROOT-CAUSE.md). Two deterministic scenarios **fail on all four RC2 SDKs and on the v0.1.30 Node client, pass on all four RC3 SDKs**. Live, against a real server: the R20 check fails with the RC2 Node client and passes on all four RC3 SDKs |
| Configuration ordering | Monotonic application verified | The ten invariants of the brief, mechanism and evidence: CONFIGURATION-CONSISTENCY.md §5. 0 I2 (rollback/resurrection) violations in 852 SDK scenarios and 40 server seeds |
| SDK reliability | No regressions | Extended tier (seeds 1–200 + 13 named, every SDK): **213/213 on each of Node, web, React Native, Flutter** (RC2: 198–199/200), and 213/213 again on Node after the final optimization. CI tier 27/27 per SDK. Live 44/44. Recovery after healing unchanged (p50 11–13 ms) |
| Randomized tests | Previously failing R20 seeds pass | 173 and 184 pass on every SDK and are mandatory in CI |
| Cross-instance consistency | Tested where supported | Revisions come from one database sequence shared by all instances. `sse_faults` randomized (writes through two instances, clients on both, instance kills, Redis outages), seeds 1–40 × 30 steps: pass, with polled revisions now checked against the database. Live restart: a second instance on the same database, SIGKILLed and restarted, per SDK: pass |
| Security | No new critical or high findings | `cargo audit`, `cargo deny`: clean. `npm audit --omit=dev` (Node, web, React Native): 0. No dependency changed. Review: the query parameter is parsed by hand (never rejected, no allocation); tombstones are environment-scoped and were already sent to the same SDK keys by the stream bootstrap; SDK parsing is strict (integer revisions, own properties only, whole response rejected otherwise) and never invents revision 0 |
| Performance | No unexplained material regressions | Server, array path: unchanged within run spread (a first candidate that cost +6–8 % CPU on this path was found and fixed). `?v=2`: +1–5 % CPU, +13 bytes per flag. SDK evaluation: unchanged. Complete-set application: +0.3–0.4 µs per flag (explained: the version comparison itself). RC3-PERFORMANCE-REGRESSION.md |
| Benchmarks | README reflects measurements | No published number changed (the public suite measures the array snapshot, evaluation and propagation, all unchanged), so the README is unchanged |
| CI | Required jobs pass on the final commit | **20/20 jobs passed**, run `44451311155` on `64c0fe8` (PR #43), including on Linux: the fault suites' CI tier with seeds 173 and 184 and the R20 scenarios (Node 60, web 57, React Native 54 tests; Flutter 27 scenarios with the Rust engine), the live suite with the new R20, concurrency and SIGKILL-restart checks on all four SDKs (0 failed checks), `revisioned_snapshot_carries_revisions_and_tombstones` (Rust tests and server integration), `sse_faults`, Docker smoke, Miri, supply chain, the Android native build |
| Compatibility | Documented version matrix | CONFIGURATION-CONSISTENCY.md §6; SDK-COMPATIBILITY.md. Old SDK + new server: unchanged (array). New SDK + old server: array, behaves as 0.1.30 (named legacy scenario passes). No revision is invented for a legacy response |
| Rust gate | | fmt, clippy `-D warnings` (workspace, all targets and features), 157 lib/bin tests (RC2: 155), `cargo doc -D warnings`: pass. Server integration suites with PostgreSQL and Redis: all pass except `reliability::g2` (Linux-only `TCP_USER_TIMEOUT`; fails on macOS by design, as for v0.1.30 and RC2) |
| Unit suites | | Node 33 (+6), web 30 (+6), React Native 27 (+6), Flutter 25 (+6), edge 12, ssr 9: pass |

## 3. Findings and limitations

| Item | Status |
|---|---|
| R20 | **Fixed** (this RC) |
| Ordering across poll and stream needs a 0.1.31 server | Documented boundary (CONFIGURATION-CONSISTENCY.md §6, CHANGELOG). Against an older server the SDK behaves exactly as 0.1.30 |
| Ordering is per flag, not across flags | Documented (§1 of the same); unchanged from earlier releases |
| A replay and a poll more than the tombstone retention (24 h) apart; a database restore or lossy failover repeating revisions | Documented (§2, §4, §8); recovery by reconnect, and `setval` guidance for restores |
| Flutter bundles no engine (F1, pre-existing) | Unchanged; in the CHANGELOG's known limitations |
| iOS not certified (React Native iOS unbuilt here; no devices) | Unchanged; in the CHANGELOG's known limitations |
| Propagation p99 (198 vs 75 ms pooled, contended host) | **Not rerun**: no dedicated Linux host this session; RC3 does not touch the measured path. Not shown resolved; in the CHANGELOG's known limitations |
| Carried from RC2: R6b, R7, R9, R10, R11, R14, R16, R17; N1, N3; the 422 limits (owner call) | Unchanged |

## 4. Decision

**GO.**

Every acceptance requirement of the brief is met with evidence, on the
final commit and in CI:

- **R20 is reproduced, explained and fixed** in the server and all four
  SDKs, with the smallest change the architecture allowed: the existing
  revision sequence and tombstones, exposed on the one path that lacked them,
  behind an opt-in parameter. Its regressions fail on the RC2 and v0.1.30
  clients and pass on RC3, deterministically, in the randomized tiers and live
  against a real server.
- **No regression**: the extended tier passes 852/852 scenarios with zero
  violations of any invariant (RC2 had five R20 failures), the live suite
  44/44, every server suite as before, and CI 20/20. Recovery times are
  unchanged.
- **No security finding** and no dependency change.
- **No unexplained performance regression**: the one regression the A/B
  found (the first candidate's +6–8 % on the old snapshot path) was removed
  before this decision; what remains is small, explained and confined to the
  new opt-in path or to once-per-reconnect work. The README's measured
  numbers are unaffected.
- **Compatibility is explicit**: old SDKs and old servers keep working;
  where a guarantee needs a 0.1.31 server, the documents and the CHANGELOG
  say so.

What remains is documented and does not depend on RC3: Flutter's missing
bundled engine, iOS certified only for Flutter on the simulator, and the
propagation p99 not yet re-measured on a dedicated Linux host (**not shown
resolved**; RC3 does not change that path). All three are in the CHANGELOG's
known limitations, which satisfies RC2's release-notes condition. The carried
low-severity items of RC2 §3 are unchanged.

GO is conditional on nothing further from RC3. It would become NO-GO only if
a later change to this branch failed CI, or if the owner decided that iOS or
the propagation re-measurement must gate v0.1.31.

## 5. Release mechanics (owner)

Not done here, and unchanged from RC2: review and merge this PR into `dev`;
date the CHANGELOG entry (the workspace is already at 0.1.31); merge
`dev → main` (the docs site now builds there); tag `v0.1.31`; publish. After
the merge to `main`, `dependency-submission.yml` should close the three stale
grpc Dependabot alerts. Recommended, not blocking: `bench/public/run.sh
propagation` on a dedicated Linux host.
