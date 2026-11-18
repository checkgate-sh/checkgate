# Baseline assessment: v0.1.31

Assessed on 2026-11-18, before any change.

| | |
|---|---|
| Baseline | tag `v0.1.30` = `main` `8ef6452`. Its tree is identical to `origin/dev` `f38f02e`, the base of this milestone (`git diff v0.1.30 origin/dev` is empty). |
| Integration branch | `feat/v0.1.31-production-readiness` (from `origin/dev`) |
| CI at baseline | `ci.yml` green on `dev` and `main` (2026-11-11). Extended reliability (`reliability.yml`, scheduled) green daily from 2026-11-12 to 2026-11-17. |
| Host for local evidence | Intel Core i5-8259U (4 cores / 8 threads), 8 GiB, macOS 15; Docker Desktop 29.7.2 (VM: 8 vCPU / 3.8 GiB); rustc 1.98.0 (pinned); PostgreSQL 16 and Redis 7 in containers |

## Architecture (as found)

| Component | Path | Role |
|---|---|---|
| Evaluation core | `core/` | The only evaluator: rules, rollout hashing (MurmurHash3), weighted variants, prerequisites. `#![forbid(unsafe_code)]`. |
| Server | `server/` (axum 0.8, sqlx/PostgreSQL, Redis pub/sub) | Control plane: REST API, dashboard, `/stream` (SSE) and `/flags/snapshot` for SDKs, impressions/events ingest, scheduler, webhooks, integrations, Prometheus `/metrics` on a separate opt-in listener. The server does **not** evaluate flags. |
| SDK bindings | `sdks/nodejs` (N-API), `sdks/web` (WASM, also used by `edge/`), `sdks/react-native` (JSI → C FFI), `sdks/flutter` (Dart FFI) | Each wraps `core`; transport, caching and reconnects are in JS/Dart. |
| Edge / SSR | `edge/`, `ssr/` | Snapshot caching and bootstrap helpers that delegate evaluation to the WASM core. |
| CLI | `cli/` (Node) | Typed accessor generation (`--check`, `--json`). |
| Integrations | `integrations/checkgate-go`, `kubernetes-operator`, `terraform-provider-checkgate` | Admin-API clients (Go). |
| Dashboard | `dashboard/` (React, Bun) | Built into `server/public/`. |
| Deployment | `Dockerfile`, `docker/entrypoint-full.sh`, `docker-compose.yml` | One image with two modes: `all-in-one` (bundled PostgreSQL and Redis) or `external`. No Kubernetes or Helm charts are shipped (the operator manages flags, not the server). |
| Benchmarks | `bench/` (Criterion), `bench/public/` (Docker Compose harness), `server/examples/{throughput,loadgen}.rs` | Public results in `docs/performance/results/v0.1.30/`. |

## Local gate at baseline (2026-11-18)

| Check | Result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -D warnings` | pass |
| `cargo test --workspace --all-features` (unit) | 129 pass |
| Server suites, with PostgreSQL and Redis: `integration`, `e2e_sse`, `rate_limit`, `setup_ownership`, `sdk_health`, `environment_isolation`, `lifecycle` | all pass |
| `reliability` | 13/14. `g2` fails on macOS by design (Linux-only `TCP_USER_TIMEOUT`); it passes in Linux CI. |
| `cargo deny check` | pass |
| `cargo audit` | **fail: RUSTSEC-2026-0538** (`quinn-udp` 0.5.14, published 2026-09-30, after v0.1.30) |

## Findings

Classification: VERIFIED · PARTIALLY VERIFIED · UNVERIFIED · CONFIRMED ISSUE · NOT APPLICABLE.

### Correctness

| Area | Class | Evidence / sources | Risk | Action |
|---|---|---|---|---|
| Deterministic evaluation and bucketing | PARTIALLY VERIFIED | `core` tests sample 1 000–10 000 users; no exhaustive or cross-release pinning | A hashing change would silently move users | Add exhaustive and pinned tests (done) |
| Rollout monotonicity | UNVERIFIED (true by construction, not tested) | `evaluator.rs`: `hash % 100 < pct` | — | Test (done) |
| Rule precedence, defaults, missing attributes | VERIFIED | 48 core tests; `docs/guide/concepts.md` | — | Fix the architecture diagram, which omitted prerequisites (done) |
| Prerequisite fan-out | **CONFIRMED ISSUE** | `prerequisites_satisfied` bounded only by depth | An editor can freeze every SDK | Fixed (C1) |
| Concurrent evaluation during updates | UNVERIFIED | `FlagStore` swaps a whole `Arc<Flag>` | — | Test (done) |
| Cross-SDK consistency | VERIFIED for decisions (one evaluator); **CONFIRMED ISSUE** for context decoding | `sdks/*/src/lib.rs` | A non-string attribute erased every attribute in web/RN/Flutter, failing open on `not_equals` | Fixed (C3) |
| Environment isolation | VERIFIED | `server/tests/environment_isolation.rs` (CI) | — | Carried forward |
| Snapshot vs SSE bootstrap consistency | **CONFIRMED ISSUE** | `stream.rs` snapshot: two statements, silent drop | Polling SDKs could lose a flag | Fixed (C2) |

### Reliability

| Area | Class | Evidence / sources | Action |
|---|---|---|---|
| Lost or late events, Redis loss, shutdown, admission control | VERIFIED | `tests/reliability.rs`, `tests/lifecycle.rs`, daily extended run | Carried forward |
| Flutter SSE framing | **CONFIRMED ISSUE** | `checkgate_flutter.dart` split chunks on `\n` | Fixed (R1) |
| Segment read failure after commit | **CONFIRMED ISSUE** | `flags.rs` `unwrap_or_default()` | Fixed (R2) |
| Flutter poll vs stream race | **CONFIRMED ISSUE** | no generation guard | Fixed (R3) |
| React Native without `EventSource` | **CONFIRMED ISSUE** | no polyfill and no docs | Fixed (R5) |
| Silent dead-stream detection in SDKs | **CONFIRMED ISSUE** | no idle timeout in any SDK | Open (R4), recommended for v0.1.32 |
| 401/403 retry policy in SDKs | **CONFIRMED ISSUE** | retries forever | Open (R6); changing it alters the API |

### Security

| Area | Class | Action |
|---|---|---|
| AuthN/AuthZ, project and environment boundaries, SDK-key confinement | VERIFIED (review and existing tests) | — |
| Segment-expansion amplification | **CONFIRMED ISSUE (High)** | Fixed (S1) |
| Webhook redirect SSRF and full-body storage | **CONFIRMED ISSUE (Medium)** | Fixed (S3) |
| Secrets in delivery errors and logs (`?sdk_key=`, integration URL) | **CONFIRMED ISSUE (Low)** | Fixed (S4, S7) |
| PAT scope/expiry edges; audit paging | **CONFIRMED ISSUE (Low)** | Fixed (S5, S6) |
| Dependency advisories | **CONFIRMED ISSUE** (`cargo audit` red) | Fixed (S9) |

### Performance (from the v0.1.30 public results; methodology in `docs/performance/benchmarks.md`)

| Metric | v0.1.30 value | Class | Note |
|---|---|---|---|
| Rust core evaluation | 79–460 ns (rollout 295 ns), 0 allocations | VERIFIED | Criterion, 3 runs |
| Node SDK `isEnabled` | ~7.1 µs | VERIFIED as measured; **contradicts the "sub-microsecond" claim** in the README, SDK READMEs, JSDoc and the docs site | Profile and optimize (done: 0.8–3.9 µs) |
| `/flags/snapshot` throughput | ~500 req/s at 1 vCPU / 512 MiB, CPU-bound | VERIFIED | Profile first |
| Propagation p99 | 21 ms (1 client) to 90–98 ms (500–1 000 clients); 0 missed | VERIFIED | README "< 50 ms" holds at p50 and up to 100 clients at p99 |

### Observability, deployment, DX

| Area | Class | Note |
|---|---|---|
| Prometheus metrics (HTTP, sync, SSE, DB pool, readiness), liveness and readiness probes, structured JSON logs | VERIFIED | `docs/operations/observability.md` |
| Process CPU and memory metrics | UNVERIFIED (none exported) | Evaluated in OBSERVABILITY-REVIEW.md |
| SDK-side evaluation metrics | NOT APPLICABLE to the server | Evaluations never reach the server; impressions are the existing opt-in signal |
| Docker all-in-one shutdown | **CONFIRMED ISSUE** | PostgreSQL and Redis SIGKILLed on stop (D1) |
| Upgrade 0.1.30 → 0.1.31 | VERIFIED low risk | No migrations since v0.1.30 (`server/migrations` unchanged), so rollback is clean as long as no migration is added |
| Kubernetes / Helm | NOT APPLICABLE | Not shipped; not added |
| README and docs performance claims | **CONFIRMED ISSUE** | "Sub-microsecond" and "< 50 ms" are stated without qualification |

## Evidence not repeated

These were judged current and were not re-run:

- The v0.1.30 propagation benchmark (75 min, Docker): the sync and stream path
  is unchanged except the R2 error branch, which does not run in the benchmark.
- The extended reliability soak: it runs daily in CI on `main`.
- Android native verification and Go integration tests: no changes in those
  areas.
