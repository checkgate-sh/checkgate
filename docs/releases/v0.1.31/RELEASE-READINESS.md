# Release readiness: v0.1.31

| | |
|---|---|
| Candidate | `feat/v0.1.31-production-readiness` (from `dev` `f38f02e` = `v0.1.30`) |
| Date | 2026-11-18 |
| Recommendation | **CONDITIONAL GO** (conditions in §12) |

## 1. Executive summary

v0.1.31 is a hardening and efficiency release with no migration, no
configuration change and no API change. Rollback to v0.1.30 is tested.

It fixes one **high** issue (one editor could exhaust server memory through
segment expansion) and eight **medium** issues:

- prerequisite fan-out could freeze every SDK;
- SSRF through webhook redirects;
- one invalid attribute erased all attributes in three SDKs, failing open;
- Flutter lost flags, and could hang `connect()`, on large bootstraps;
- a failed segment read published stripped flags;
- Argon2 on the async runtime let logins stall an instance;
- React Native hung without an `EventSource` polyfill;
- the all-in-one container killed PostgreSQL on every stop.

It also fixes several low-severity issues and a new RustSec advisory.

Node SDK evaluation is **1.9–8.8× faster**, and server CPU per snapshot
request is **33 % lower**, both measured against v0.1.30 on the same host.
Every performance claim in the docs now matches a measurement.

No critical or high finding remains open. The main open risks are
pre-existing SDK behaviours (no dead-stream watchdog; retrying 401 forever).
They are documented and deferred because fixing them changes public
behaviour.

## 2. Implementation changes

| Area | Change | Files |
|---|---|---|
| Core | Prerequisite evaluation budget (1 000 per evaluation, fails closed); lenient attribute decoder; rollout pinning, monotonicity and concurrency tests | `core/src/evaluator.rs`, `core/src/attributes.rs` |
| Server: limits | Rules, segment references (no duplicates), prerequisites (no self or duplicates), variants and segment-rule caps → 422 | `server/src/limits.rs`, `api/flags.rs`, `api/segments.rs` |
| Server: sync | No publish of segment-stripped flags; resync instead | `api/flags.rs` (`publish_flag_change`), `metrics.rs` |
| Server: snapshot | One statement (consistent and one round trip); fail on undecodable flag; direct `jsonb` → `Flag` decode | `stream.rs`, `api/segments.rs` |
| Server: security | Webhook client without redirects and with a 1 KiB body cap; URLs stripped from errors and logs; PAT expiry cap and revoke scope; audit paging clamp; path-only request span; Argon2 on the blocking pool | `webhook_fire.rs`, `integrations.rs`, `auth.rs`, `api/tokens.rs`, `api/audit.rs`, `api/impressions.rs`, `main.rs`, `api/session.rs` |
| Server: observability | Linux process metrics | `process_metrics.rs` |
| SDKs | Node: referenced-attribute conversion. Web/RN/Flutter: lenient attributes. Flutter: SSE framing and poll guard. RN: missing-`EventSource` fallback and docs | `sdks/*` |
| Deployment | All-in-one supervisor with clean shutdown; compose `stop_grace_period`; smoke check for clean shutdown; smoke script portable to bash 3.2 | `docker/entrypoint-full.sh`, `docker-compose.yml`, `scripts/docker-smoke.sh` |
| Supply chain | `quinn-udp` 0.5.16 | `Cargo.lock` |
| Docs / DX | Claims corrected; RN polyfill; diagram; A/B scripts; release records | `README.md`, `docs/**`, `bench/ab/`, `docs/releases/v0.1.31/` |

## 3. Security findings and disposition

See SECURITY-REVIEW.md.

| Severity | Found | Fixed | Open |
|---|---|---|---|
| Critical | 0 | 0 | 0 |
| High | 1 (S1) | 1 | 0 |
| Medium | 4 (S2, S3, S8, S10) | 4 | 0 |
| Low | 5 (S4–S7, S9 info) | 5 | 0 |
| Deferred (low / info) | N1–N6 | — | 6, documented with mitigations |

GitHub Dependabot's 3 open grpc alerts (Terraform provider) are stale:
v1.85.0 is pinned, at or above every patched version. See SECURITY-REVIEW.md.

## 4. Correctness evidence

See CORRECTNESS-REVIEW.md. Determinism and monotonicity were tested
exhaustively (2 000 users × 101 percentages); 7 buckets are pinned against
v0.1.30 (hashing is byte-identical); evaluation during concurrent updates
never mixes versions; a 360 000-call Node differential found 0 mismatches.
Three confirmed issues were fixed (C1–C3), each with a regression test.

## 5. Reliability evidence

See RELIABILITY-REVIEW.md and OPERATIONAL-FAILURE-TESTS.md. Locally, the
`reliability` suite passed 13/14 (`g2` is Linux-only by design) and
`lifecycle` 8/8; the extended reliability run has been green daily in CI.
Seven findings were fixed (R1, R2, R3, R5, R8, R18, D1); 12 are open, all
medium or lower, with dispositions.

## 6. Performance comparison

See PERFORMANCE-COMPARISON.md.

| Metric | v0.1.30 | v0.1.31 |
|---|---|---|
| Node `isEnabled`, boolean, 5 attributes | 6 857 ns | **778 ns** |
| Node, 10 rules | 7 412 ns | 3 895 ns |
| Snapshot server CPU per request (native) | 2 634 µs | **1 757 µs** |
| Snapshot throughput (native, concurrency 10) | 442 req/s | 635 req/s |
| Snapshot decode, 101 flags (Criterion) | 486 µs | 154 µs |

Propagation was not re-measured; the path is unchanged.

## 7. Resource usage

RSS unchanged, about 10 MiB natively under snapshot load and 13.3 MiB in the
container after boot. No growth was observed. Three unbounded allocations
were removed: segment amplification, webhook body reads, and concurrent
Argon2 working sets. See CPU-AND-MEMORY-PROFILING.md.

## 8. Deployment verification

See DEPLOYMENT-READINESS.md and UPGRADE-AND-ROLLBACK.md.

- `scripts/docker-smoke.sh` passes all scenarios on the candidate image,
  including the new clean-shutdown check, which **fails on the v0.1.30 image**.
- Upgrade, rolling upgrade and rollback were tested on one database with real
  release binaries.

## 9. SDK compatibility

See SDK-COMPATIBILITY.md. All four SDKs' unit suites pass. The real engine
was exercised through the Node native tests, the WASM smoke test and the
Rust FFI tests (Miri in CI). The Node SDK e2e passes against the v0.1.31
server. The wire protocol is unchanged in both directions.

## 10. Remaining limitations

- The README benchmark graphic still shows **v0.1.30** numbers (labelled as
  such). Refreshing it needs the 75-minute public suite on the release commit.
- The v0.1.31 server throughput gain is measured **natively** (latency-bound
  setup). The CPU-bound 1 vCPU gain (~1.5×) is a projection from CPU per
  request.
- R2 (segment read failure) is verified by review, not by fault injection.
- iOS builds of the RN and Flutter SDKs, and live e2e runs of web, RN and
  Flutter against a server, were not performed.
- The `setup_ownership` load flake: 2 failures in 16 runs before the Argon2
  fix and 0 in 10 after. The mechanism fits, but the logs of the failing runs
  were not kept, so the root cause is not proven.

## 11. Unresolved risks

| Risk | Severity | Mitigation |
|---|---|---|
| SDKs keep serving stale flags on a silently dead connection (R4) | Medium | Pre-existing. Server keep-alive every 15 s; recommended first item for v0.1.32 |
| SDKs retry 401/403 forever; `connect()` never settles with a revoked key (R6) | Medium | Pre-existing; backoff caps the load |
| Webhook targets can be private addresses set directly by an admin (N1) | Low | Admin only; egress filtering |
| Open streams survive key revocation (N3) | Low | Restart instances after an urgent revocation |
| The all-in-one supervisor runs as root | Low | Idle shell, no network or input; services run non-root |
| New 422 limits reject an unusual but legitimate configuration | Low | Limits far above observed use; pre-upgrade query provided |

## 12. Recommendation: CONDITIONAL GO

The evidence supports releasing v0.1.31. Every confirmed critical or high
finding is fixed with a regression test, nothing regresses against v0.1.30 in
any measured dimension, and upgrade and rollback are tested.

Conditions:

1. **CI green on the phase PRs.** In particular, the Linux-only checks that
   could not run on this macOS host: `reliability::g2`, `docker-smoke` on
   Linux, `miri`, the Android native build, and the new native-attributes
   step.
2. **The owner accepts two judgement calls:** the 422 limit values, and the
   root PID 1 supervisor in the all-in-one image.
3. **Recommended, not blocking:** run `bench/public/run.sh all` on the release
   commit to publish v0.1.31 benchmark results and refresh the README graphic.

Release mechanics are unchanged and left to the owner: bump the version
(`scripts/bump-version.sh 0.1.31`), date the CHANGELOG entry, merge
`dev → main`, tag `v0.1.31`.

## Gate results (final tree, local, 2026-11-18)

macOS host; PostgreSQL 16 and Redis 7 in containers.

| Check | Result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -D warnings` | pass |
| `cargo test --workspace --all-features` (unit, doc) | **197 passed**, 0 failed (baseline: 129) |
| `cargo doc` with `-D warnings` | pass |
| Server suites with a database: integration 1/1, e2e_sse 1/1, rate_limit 5/5, setup_ownership 9/9, sdk_health 1/1, environment_isolation 1/1, lifecycle 8/8, **hardening 4/4 (new)** | pass |
| `reliability` | 13/14: `g2` fails on macOS by design (Linux `TCP_USER_TIMEOUT`), same as baseline |
| `cargo audit` / `cargo deny check` | pass / pass |
| `scripts/check-tracked-artifacts.sh`, `bench/public/scripts/validate.py` | pass / pass |
| Node SDK (`node --test`, incl. 6 native) / CLI / edge / ssr | 27 / 57 / 12 / 9, all pass |
| Web / React Native (vitest) | 21 / 21 pass |
| Flutter (`flutter test`) | 17 pass |
| Dashboard (`bun run build`, `bun run test`) | build OK; 33 pass |
| `scripts/docker-smoke.sh` on the candidate image | all scenarios pass, incl. the new clean-shutdown check |
| Node SDK live e2e against the v0.1.31 server | pass |
| WASM build + `smoke-core.mjs` | pass |
| Upgrade / rolling / rollback (`bench/ab/upgrade-rollback.sh`) | pass |

Not run locally (Linux or device only; CI covers the first four): Miri,
Android native build, Go integrations (unchanged), Docker smoke on Linux, iOS
builds.
