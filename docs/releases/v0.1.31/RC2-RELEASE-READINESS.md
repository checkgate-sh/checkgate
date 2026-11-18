# Release readiness: v0.1.31 RC2

| | |
|---|---|
| Candidate | `fix/v0.1.31-rc2-reliability`, from `dev` `ba99a4f` (the v0.1.31 milestone, PR #33–#40 merged) |
| Date | 2026-11-19 |
| Previous decision | CONDITIONAL GO (RELEASE-READINESS.md, 2026-11-18) |
| Decision | **CONDITIONAL GO** (§4) |

RC2 set out to close the conditions and open risks of the first decision:
SDKs that never noticed a dead stream (R4) and retried a rejected key forever
(R6), fault testing of the reconnect path, the segment-read failure verified
only by review (R2), SDK e2e runs never made for web, React Native and
Flutter, iOS, and a README graphic still showing v0.1.30.

## 1. What RC2 changed

| Change | Commit(s) | Evidence |
|---|---|---|
| Heartbeat as a named SSE event (`event: heartbeat`, `data: {}`), interval announced in `connected` (`heartbeat_ms`), `SSE_HEARTBEAT_INTERVAL_SECS` 1–300 (default 15) | `22dcd6e` | `stream.rs` unit tests (exact bytes); sse_faults; live suite |
| All four SDKs: connect deadline (30 s) and idle deadline (3 × heartbeat), reconnect through the existing backoff; 401/403 terminal with `CheckgateAuthError` / `CheckgateAuthException`; `connect()` always settles; `getStatus()` / `status` | `8f8bbac` | SDK-CONNECTION-RELIABILITY.md |
| **R19** (React Native never saw a stream error; one blip = stale until restart) | `8f8bbac` | fault suite: 19 of 21 scenarios failed before, 0 after |
| R12 (Flutter double loop), R13 (web stream after early `disconnect()`), one WASM engine per web client | `8f8bbac` | unit tests |
| Shared randomized SSE fault suite for every SDK; server-level fault suite including C1 | `7445450`, `ba2c4cf` | SSE-FAULT-INJECTION.md |
| Live e2e of every SDK against a real server | `5aacd09` | SDK-END-TO-END-VALIDATION.md |
| **F1** Flutter package importable (`lib/`) | `5f8ef48` | fresh `flutter create` app resolves the import |
| All-in-one PID 1 supervisor drops every capability except KILL, SETUID, SETGID; no new privileges | `b0dacd6` | `docker-smoke.sh` checks CapEff/CapBnd `0xe0`, NoNewPrivs 1 |
| Dependabot: the 3 grpc alerts are stale; the Go module graph is now submitted from CI | `0b71358` | SECURITY-REVIEW.md |
| Public benchmarks refreshed for v0.1.31 against v0.1.30 in the same session; cost suite; README sections | `cee868f`, `66cbfec` and the RC2 benchmark commits | PUBLIC-BENCHMARK-RESULTS.md, COST-EFFICIENCY-BENCHMARKS.md |
| CI runs the fault suites on every PR, new seeds nightly | `3c32cbe` | `ci.yml`, `reliability.yml` |

No migration. Public API changes are additive except that `connect()` now
rejects where it used to hang forever (rejected key; `disconnect()` before the
first sync); listed under "Upgrading from 0.1.30" in the CHANGELOG.

## 2. Evidence

All measured on this macOS (Intel) host unless marked CI; nothing below is
projected except where a row says so.

| Area | Result |
|---|---|
| SDK fault suite, CI tier (10 named + seeds 1–12, every SDK) | **22/22 on each of Node, web, React Native, Flutter** (Flutter with the real Rust engine) |
| SDK fault suite, extended (seeds 1–200 + named, every SDK, about 9,750 random steps each) | Node 199/200, web 198/200, React Native 199/200, Flutter 199/200. **Every failure is R20** (pre-existing). Silent stream replaced in 296–343 ms (100 ms heartbeat); recovery after healing p50 11–14 ms, max 419–654 ms; never more than 1 stream |
| Old clients against the same suite | v0.1.30 Node: half-open never detected (seeds 2, 11, 12), revoked key never reported. Previous React Native: 19 of 21 scenarios failed (R19) |
| Server fault suite (`sse_faults`, real PostgreSQL and Redis) | 5/5: quiet stream kept; stranded stream replaced 3× within 6 s; revoked key terminal and recoverable; randomized seeds 1–3 (CI) and **1–40 × 30 steps** converge (p95 1.97 s, max 2.92 s after healing) |
| C1, segment read failure (R2), executed | pass: no stripped flags ever served, snapshot answers 500, both instances converge 446 ms after the table returns. With the v0.1.30 publish restored, the test fails (rev 2 with `rules: []`) |
| Live e2e, every SDK against a real server (final commit `3c32cbe`) | **32/32** (8 checks × 4 SDKs): bootstrap, targeting, patch, delete, reconnect, half-open detected in 3.07–3.15 s (1 s heartbeat), poll fallback and return, invalid key (1 request; web 2), revoked key terminal, clean shutdown |
| iOS | Flutter app on the iOS 17 simulator: pass (connect 97 ms, live update 118 ms, auth exception). Rust engines cross-compile for 3 iOS targets with every symbol; RN JSI host links. RN app build, `Checkgate.mm`, devices: **not run** |
| Docker all-in-one | `docker-smoke.sh` all scenarios pass; supervisor CapEff/CapBnd `0xe0`, NoNewPrivs 1 |
| Unit suites (final tree) | Node 55, web 46, React Native 43, Flutter 19, edge 12, ssr 9: all pass |
| Rust gate | fmt, clippy `-D warnings`, 155 lib/bin tests, doc, `cargo audit`, `cargo deny`: pass. Server suites with a database: pass, except `reliability::g2` (fails on macOS by design, as for v0.1.30) |
| Benchmarks: v0.1.31 vs v0.1.30, same session, alternated | Node SDK eval 7.2× faster (rollout), 8.8× (boolean); server CPU per snapshot −21 % and throughput +21 % at 1,000 clients, unchanged at 10; RSS unchanged; propagation p50 equal, p99 pooled 198 vs 75 ms (two contended runs; median run 84 vs 76 ms); 0 of 1,921,200 deliveries missed. PUBLIC-BENCHMARK-RESULTS.md |
| Cost suite (measured, calculated, projected kept apart) | 10,000 streams idle at 14.7 % of a core, 448 MiB; 1,000 clients: 8.7 GB a month streaming against 669–4,015 GB polling (calculated); $38.56 a month projected for 100–10,000 streaming clients. COST-EFFICIENCY-BENCHMARKS.md |
| `bench/public/scripts/validate.py`, `scripts/check-tracked-artifacts.sh` | pass |
| CI on the final commit | pending at the time of writing; recorded below once the PR run finishes |
| Dependabot | 3 grpc alerts are stale (pinned v1.85.0 ≥ every fix); they close when `dependency-submission.yml` runs on `main` |

## 3. Findings still open

| ID | Finding | Severity | Why it does not block |
|---|---|---|---|
| R20 | A delayed stream replay can briefly roll back flags polled meanwhile (found by the extended tier; RELIABILITY-REVIEW.md) | Medium | Pre-existing (v0.1.30 fails the same seed). Needs poll fallback active **and** a new stream whose bytes lag more than a poll interval; the state rolls back for as long as that stream takes to catch up, then corrects itself. The fix is a backward-compatible protocol addition across the server and four SDKs. **Owner decision** whether it goes into v0.1.31 |
| F1 (rest) | The Flutter package bundles no engine | High for Flutter adopters | Pre-existing in every published version. The import is fixed and the verified linking steps are documented; a plugin that builds the engine is a feature |
| iOS | React Native on iOS not built (CocoaPods broken on this host); no device run for either SDK | — | Flutter ran on the iOS simulator; the RN JSI host and both Rust engines cross-compile and link for all three iOS targets. iOS is **not certified** and the release notes must say so |
| Propagation tail | v0.1.31's pooled p99 at 1,000 clients is 198 ms against 75 ms, from two contended runs | — | Runs 3–6 overlap; isolation runs show no difference; follow-up: rerun on a dedicated Linux host |
| Carried | R6b, R7, R9, R10, R11, R14, R16, R17; N1, N3; the 422 limits (owner call) | Low | Unchanged from RELEASE-READINESS.md |

## 4. Decision

**CONDITIONAL GO.**

RC2 met what it set out to do. Every SDK now notices a dead stream within
three heartbeats and recovers, stops on a rejected key and says so, and has
`connect()` settle. React Native's stream errors (R19) and Flutter's import
(F1) are fixed. R2 is verified by fault injection, and all four SDKs ran
end to end against a real server. The README carries v0.1.31 numbers measured
against v0.1.30 in the same session. Nothing in RC2 regresses v0.1.30 in any
measured dimension. The propagation tail is the one measured difference, and
the evidence attributes it to the host, not the release.

It is not an unconditional GO because the release would still ship with:

1. **R20 open** (medium, pre-existing, all SDKs). The fix touches the
   snapshot protocol and all four SDKs, more than an RC change should without
   the owner's agreement.
2. **Flutter without a bundled engine** (F1, pre-existing), and **iOS not
   certified** (React Native on iOS not built here).

Conditions for release:

1. CI green on the final RC2 commit.
2. The owner decides R20: ship v0.1.31 with R20 listed as a known issue, or
   fix it first (estimated scope: one server change to `/flags/snapshot`, the
   same change in four SDKs, the two seeds moved into the CI set).
3. The release notes state: Flutter needs the engine linked as documented;
   iOS is verified on the simulator for Flutter only; R20 if not fixed.

Recommended, not blocking: rerun `bench/public/run.sh propagation` on a
dedicated Linux host to settle the propagation tail; size servers that hold
10,000 streams at more than 512 MiB.

GO would follow from conditions 1–3 with R20 fixed or accepted. NO-GO would
follow from a CI failure that is a defect rather than infrastructure, or from
a decision that R20 must be fixed and the fix not yet made.

## 5. Release mechanics (owner)

Unchanged and not done here: review and merge the RC2 PR into `dev`;
`scripts/bump-version.sh 0.1.31` is already the workspace version, so only
date the CHANGELOG entry; merge `dev → main`; tag `v0.1.31`; publish. After
the merge to `main`, `dependency-submission.yml` runs and the three stale grpc
Dependabot alerts should close (dismiss them as "fix already applied" if they
do not).
