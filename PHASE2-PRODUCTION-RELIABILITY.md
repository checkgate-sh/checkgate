# Phase 2 — Production Reliability, Resilience & Observability

Engineering report for branch `feat/phase2-production-reliability` (base `dev`).

## 1. Executive summary

Checkgate's multi-instance synchronization now has a demonstrated recovery
path for every event-loss mode found, including the one the existing design
could not recover from: **a server that dies after committing a change but
before publishing it** left every other server's clients permanently stale.
A small per-instance reconciliation loop (revision index compared with
PostgreSQL, jittered, targeted per environment) closes that gap without new
infrastructure. A ping-based liveness check closes a second permanent-staleness
gap (a Redis subscription that stays open but delivers nothing).

Also delivered: graceful shutdown that actually completes (it hung on any open
SSE stream before), liveness/readiness endpoints, Prometheus metrics on a
private listener, SSE admission control and reconnect-storm damping, a
transport timeout for clients that stop reading, a 14-test multi-instance
fault-injection suite plus an 8-test lifecycle suite in CI, Criterion and
control-plane benchmarks with measured results, operations documentation,
a non-root container, opt-in RFC 7239 `Forwarded` support, and fixes for both
jobs failing on `dev` after PR #21 (Supply chain, Docker smoke).

Not done in this phase (with reasons in §16): Redis-backed distributed rate
limits, SDK-key-aware rate limits, cross-instance SDK-health aggregation, and a
Flutter test run (no Flutter toolchain on the machine). No serious finding is
left unresolved; the remaining risks are listed in §14.

## 2. Commits

| | |
|---|---|
| Starting commit | `28eb233` (dev, merge of PR #21) |
| Ending commit | the PR head (last code commit `aaef95c`; this report is in the commits after it) |

## 3. Existing architecture assessment

Write path: handler commits to PostgreSQL (global revision sequence; deletes
leave a one-day tombstone) → post-commit audit log, segment expansion →
`PUBLISH` on Redis → every instance's subscriber → per-instance broadcast
channel (256) → SSE streams, filtered by environment. SDKs stage the bootstrap
until `ready`, and drop events older than the revision they hold.

Recovery paths present at `28eb233`:

| Path | Covers |
|---|---|
| Publish retry (1 quick retry) → local delivery + `RESYNC` retry loop | Redis down at publish time, *if the publisher survives until Redis returns* |
| Resync on subscriber resubscribe | Subscription that *errors* and reconnects |
| Lagged receiver → close | Slow consumer whose stream is still polled |
| SDK reconnect / poll fallback | Client-side network loss |

Gaps (confirmed in §4): nothing consulted PostgreSQL after bootstrap, so any
loss not observed by a *surviving* process was permanent; the subscriber had no
liveness check; shutdown and slow-reader handling had no bound.

Consistency guarantee before: eventual delivery only when the publishing
process and every subscriber connection survive or visibly fail — not stated
anywhere, and not true for crashes. After: see §6.

## 4. Confirmed failure modes

Reproduced against a baseline binary built from `28eb233`
(`CHECKGATE_TEST_SERVER_BIN=<baseline> cargo test -p server --test reliability
-- --test-threads=1`) and with direct probes. Baseline run: 9 of 13 scenarios
passed; the four failures are R1–R3 plus a fixture bug in J (a slug clash,
fixed in the test).

### R1 — Publisher crash between commit and publish → permanent staleness (P0)

- **Previous behavior:** another instance's clients never received the change;
  nothing ever repaired it short of a reconnect for an unrelated reason.
- **Reproduced:** `f2_publisher_killed_between_commit_and_publish` — instance
  A's Redis link is blackholed so its publish hangs; the PATCH commits (polled
  in PostgreSQL); A is SIGKILLed. Baseline: client on B still at revision 1
  after 20 s (`sdk-b: lost: rev 1, db 2`). Same for a commit with no event at
  all (`f3_…`: SQL update + delete; baseline never converged).
- **Root cause:** Redis Pub/Sub is at-most-once; the only holder of the event
  (and of the `RESYNC` retry loop) was the dead process; nothing compared
  delivered state with PostgreSQL.
- **Change:** `server/src/sync.rs` — `RelayIndex` (newest relayed revision per
  flag, seeded from PostgreSQL) and `run_reconciler` (every
  `SYNC_RECONCILE_INTERVAL_SECS`, ±20 % jitter; one `REPEATABLE READ` scan of
  `(env, key, revision)` for flags and tombstones). A committed version not
  relayed within `SYNC_RECONCILE_GRACE_SECS` → resync **only that
  environment's** clients (`{"type":"RESYNC","env_id":…}`, local only), metric
  + `WARN`. Suspects are judged against the version first noticed, so busy
  flags do not false-alarm; flags never seen count as deleted; events from
  pre-revision servers adopt the committed revision (rolling upgrades).
  Migration `…28_archive_keeps_revision.sql`: archive/unarchive (no event,
  nothing evaluable changed) no longer takes a revision — otherwise every
  archive would read as a lost change.
- **Regression tests:** `f2_…`, `f3_…`, `lifecycle::reconciliation_resyncs_only_the_affected_environment`,
  `lifecycle::relayed_changes_archival_and_segment_edits_cause_no_resync`,
  11 unit tests in `sync.rs`.
- **Verified:** F2 recovered in 3.4–3.7 s, F3 in 3.6–4.2 s (test settings 1 s
  / 2 s); only the affected environment reconnected; no false resyncs over
  several passes with archival and segment edits.

### R2 — Silently dead subscriber connection → permanent staleness (P0)

- **Previous behavior:** a subscription whose TCP connection stops delivering
  without closing (lost NAT/LB state, half-open) was never noticed.
- **Reproduced:** `e2_silently_dead_subscriber_connection_is_detected` — B's
  existing Redis connections stranded (bytes dropped, nothing closed). Baseline:
  B's client stuck at revision 1 after 20 s.
- **Root cause:** the subscriber only reacted to stream end/error; no liveness
  probe.
- **Change:** subscriber split into sink/stream; `PING` every
  `REDIS_PING_INTERVAL_SECS` with timeout ≤ 5 s; failure → reconnect →
  resync. (Reconciliation also catches it independently.) An initial version
  of this check typed the reply as a string; subscribed RESP2 connections
  answer `["pong",""]`, so every ping "failed" — caught by test A's
  no-reconnect assertion before commit and fixed.
- **Verified:** recovered in 1.6 s.

### R3 — Graceful shutdown never completed with an open SSE stream (P1)

- **Previous behavior:** SIGTERM stopped accepting, then waited forever for
  SSE responses to end; every shutdown with a connected SDK ended in SIGKILL.
- **Reproduced:** probe script: baseline binary with one `/stream` open, SIGTERM
  → still running after 30 s. Branch: exited in 0.11 s.
- **Change:** `server/src/lifecycle.rs` + `main.rs`: drain (readiness 503,
  new streams 503) → `SHUTDOWN_DRAIN_DELAY_SECS` → cancel token ends every
  stream and background task (subscriber, reconciler, scheduler, resync retry,
  limiter eviction) → in-flight requests bounded by
  `SHUTDOWN_GRACE_PERIOD_SECS` → pool close.
- **Regression test:** `lifecycle::sigterm_drains_closes_streams_and_exits_within_grace`.

### R4 — Client that stops reading holds its stream forever (P1)

- **Previous behavior:** when a client stops reading, its socket fills and
  hyper stops polling the SSE stream; the lag check never runs; connection,
  task and health-tracking entry are held indefinitely.
- **Reproduced:** `g2_client_that_never_reads_again_is_released` failed before
  the fix (still registered after 60 s).
- **Change:** `TCP_USER_TIMEOUT` (default 60 s, Linux, `socket2`) on accepted
  sockets via `ListenerExt::tap_io`; the kernel aborts connections whose data
  stays unacknowledged, including behind a zero window.
- **Verified:** with a 5 s timeout the stream was released before the flood
  that stalled it finished (test total 15 s).
- Separately confirmed *not* a defect: a client that resumes after falling
  behind is disconnected by the lag check (`g_slow_consumer_…`, passes on
  baseline).

### R5 — Dashboard files served without security headers (Phase 1 follow-up)

- **Previous behavior:** `fallback_service` was added after the layers, so
  static files bypassed security headers, CORS and rate limiting.
- **Reproduced:** probe on baseline: `/` had no `X-Content-Type-Options` or
  CSP.
- **Change:** fallback added before the layers; dashboard paths get a CSP
  allowing exactly what the built `index.html` loads (Google Fonts); dashboard
  and unknown paths are classed rate-limit-exempt, preserving prior behavior.
- **Regression test:** `lifecycle::dashboard_files_get_security_headers`, smoke
  script header check, `api::dashboard_path_tests`.

### R6 — Lost slot / tracking entry for an unpolled stream (latent)

The connection guard was created inside the stream body, so a response dropped
before its first poll leaked the `connected_clients` entry. The guard (now also
holding the capacity slot) is created before the stream and moved in.

### R7 — CI failures on `dev` after PR #21

- **Supply chain:** RUSTSEC-2026-0382 (rustls-webpki), -0381 (webpki-roots),
  -0386 (tokio `AsyncFd`, unsound). Fixed by lockfile updates (0.103.16,
  1.0.11, 1.54.1). `cargo deny check` and `cargo audit` pass.
- **Docker smoke:** all-in-one stopped at `pg_ctl start` — `/var/run/postgresql`
  existed only as a build-time artifact of the package install; CI's BuildKit
  gives a fresh `/run`. Reproduced locally with `--tmpfs /run`. The entrypoint
  now creates the directory on every start; the smoke test runs all-in-one with
  `--tmpfs /run`.

### R8 — Found by the new checks, fixed before commit

- Benchmarks first declared as `[[bench]]` in `core/Cargo.toml` broke the
  packed React Native SDK (its bundler copies the core manifest without
  `benches/`); found by `verify-android-native.sh`. Moved to an unpublished
  `bench/` crate.
- `docker-smoke.sh`'s new UID helper failed silently under `pipefail`; fixed.

## 5. Changes implemented

| Area | Change |
|---|---|
| Sync recovery | `sync.rs`: relay index, reconciliation, targeted resync, subscriber ping, `REDIS_CHANNEL`; events parsed once per instance (`Arc<SyncEvent>` broadcast) instead of once per connection |
| Reconnect storms | Resync closes spread over `SSE_RESYNC_SPREAD_MS`; bootstraps + snapshots share `SSE_BOOTSTRAP_CONCURRENCY` slots with bounded wait → `503` + jittered `Retry-After` |
| Admission | `SSE_MAX_CONNECTIONS` (10 000); refusals by reason in metrics |
| Lifecycle | `/health/live`, `/health/ready`, drain + grace shutdown, cancellable background tasks, pool `acquire_timeout` 10 s |
| Transport | `TCP_USER_TIMEOUT_SECS` |
| Metrics | `metrics.rs`, `prometheus-client`, `METRICS_ADDR` listener (off by default) |
| Container | No process runs as root; `--user 10001:10001` supported in external mode; `HEALTHCHECK` on `/health/live`; no credential `ENV` |
| Proxies | `TRUSTED_PROXY_HEADER=forwarded` (RFC 7239), never combined with XFF |
| Tests | `tests/common/{chaos,sdk,cluster}.rs`, `tests/reliability.rs` (14), `tests/lifecycle.rs` (8) |
| Bench | `bench/benches/evaluation.rs` (Criterion + allocation profile), `server/examples/loadgen.rs` |
| CI | Lifecycle + reliability in PR CI with artifacts; pinned service images; `reliability.yml` nightly/on-demand (soak ×N, load, benches) |
| Docs | `docs/operations/{reliability,observability,failure-recovery}.md`, `docs/performance/{benchmark-methodology,baseline}.md`, self-hosting, API reference, CHANGELOG |

## 6. Synchronization guarantees

**Bounded staleness once dependencies are reachable.** With PostgreSQL
reachable from an instance, a change committed at *t* is held by every client
connected to that instance by *t* + `SYNC_RECONCILE_INTERVAL_SECS`×1.2 +
`SYNC_RECONCILE_GRACE_SECS` + `SSE_RESYNC_SPREAD_MS` + one SDK reconnect
(≈ 51 s with defaults), regardless of what happened to the event. Normal
delivery is milliseconds (§9). **Not** provided: exactly-once or ordered
delivery (revisions make duplicates/reordering safe), convergence while an
instance cannot reach PostgreSQL (it reports not ready), or instant repair.
Demonstrated by the scenarios in §7; not demonstrated: Redis Cluster/Sentinel
failover, network partitions between instances and PostgreSQL lasting longer
than the test windows.

## 7. Fault-injection scenarios and outcomes

Real server processes, real PostgreSQL 16.15 and Redis 7.4.11, simulated SDK
clients following the Node SDK's revision/staging/reconnect rules, per-instance
TCP fault proxies. Recovery windows from the full workspace run (test settings:
reconcile 1 s / grace 2 s, ping 1 s, spread 200 ms; SDK backoff 200 ms–2 s).

| # | Scenario | Baseline `28eb233` | Branch | Recovery |
|---|---|---|---|---|
| A | Update via A reaches A and B clients; evaluated with checkgate-core; no reconnects | pass | pass | 0–2 ms after write |
| B | 80 concurrent writes over 2 instances, 4 clients | pass | pass | 2 ms |
| C | 15 rounds update×3 racing delete; delete→recreate | pass | pass | 6 ms / 2 ms |
| D | Redis down for whole cluster during writes | pass | pass | 481 ms after heal (repaired during the outage by reconciliation) |
| E1 | One instance's subscription down | pass | pass | 1.3 s |
| E2 | Subscription silently dead | **fail** | pass | 1.6 s |
| F1 | Instance SIGKILLed, writes, restart; new client bootstraps | pass | pass | 0.4 s |
| F2 | Publisher SIGKILLed between commit and publish | **fail** | pass | 3.7 s |
| F3 | Commit with no event (SQL update + delete) | **fail** | pass | 3.6 s |
| G | Stalled reader beside a fast one, 600 × 48 KB events | pass | pass | fast client 2 ms; stalled stream closed on resume |
| G2 | Reader that never resumes | **fail** (held > 60 s) | pass | released within the flood (5 s timeout) |
| H | 6 clients through cut-able links, 200 random writes/deletes/creates, links cut every 150 ms | pass | pass | 105 ms |
| I | Link severed ×3; down 2 s; streaming blocked → poll fallback | pass | pass | 0.15–0.2 s; 0.48 s; 0.63 s |
| J | Same keys in two environments, clients on both instances; event and snapshot isolation | pass* | pass | – |

\* J failed on baseline only because of a fixture bug (slug clash), fixed.

Lifecycle suite (`tests/lifecycle.rs`, all pass): readiness vs DB/Redis loss,
`READY_REQUIRES_REDIS`, SIGTERM drain/exit, metrics validity/cardinality/
privacy, SSE capacity 503 + recovery, static headers, no false resyncs, targeted
resync.

## 8. Metrics and health endpoints

See `docs/operations/observability.md` (full table, interpretation matrix,
alert rules). `GET /metrics` on `METRICS_ADDR` only; 28 metric families; labels
are route templates, status classes and small enums; the test asserts no env
id, SDK key, flag key or address appears and the series count stays < 600.
Health: `/health/live` (never dependency-bound), `/health/ready` (DB required,
Redis optional, draining → 503), `/health` unchanged.

OpenTelemetry: metrics are OpenMetrics, scrapeable by an OTel collector's
Prometheus receiver; tracing already uses `tracing` spans. A native OTLP
exporter was not added (scope).

## 9. Performance benchmark results

Full tables, environment and raw files: `docs/performance/baseline.md`. One
machine (i5-9400F, `powersave`), two repetitions.

- **Core:** boolean lookup+evaluation 47–52 ns, flat 10 → 10 000 flags;
  missing 28 ns; 10-rule targeting 259 ns; rollout 235 ns; multivariate 303 ns;
  5-flag prerequisite chain 254 ns; 0 allocations except rollout (2) and
  multivariate (4); 288 B per stored flag; no growth on replacement; 6 readers
  48.6 M evals/s aggregate; one SDK event apply 5.5 µs.
- **SSE:** 100 / 1 000 / 5 000 connections over 2 instances admitted in 0.07 /
  0.6 / 2.7 s, 0 failures; ~27 KB server memory per stream.
- **Propagation (write → visible, p50 / p99):** 29 / 44 ms (100),
  40 / 69 ms (1 000), 53 / 140 ms (5 000); 0 missed deliveries out of 1.22 M.
- **Breakdown:** server PATCH 25–26 ms, of which Redis publish 0.8 ms; with
  `synchronous_commit=off` PATCH p50 drops to 1.9 ms — WAL fsync dominates
  writes on this disk. Fan-out ≈ 18 µs per stream per event.
- **REST / snapshot:** list p50 3.5–5.7 ms; PATCH p50 24–28 ms; snapshot
  3 016–3 082 req/s at 32 concurrency, p50 10 ms.

## 10. Resource usage and backpressure findings

| Concern | Finding | Action |
|---|---|---|
| A. Bootstrap memory | Bootstrap materialises all of an env's events before streaming; ~100 flags ⇒ small. Large environments × reconnect waves could spike memory. | Bounded by bootstrap slots (default 5 concurrent per instance). Streaming the snapshot row-by-row deferred (§16). |
| B. Many connections | ~27 KB/stream measured | `SSE_MAX_CONNECTIONS` cap; documented sizing |
| C. Reconnect storms | RESYNC closed every stream at once; SDK jitter only 30 % of 3 s | Spread window + bounded bootstrap queue + jittered `Retry-After`; targeted env resyncs |
| D. Unbounded retry loops | `RESYNC` retry loop infinite and uncancellable | Cancellable on shutdown, per-attempt timeout, capped backoff; all loops take the shutdown token |
| E. Slow consumers | Stalled readers held forever (R4) | `TCP_USER_TIMEOUT` |
| F. Capacity | No cap | `SSE_MAX_CONNECTIONS`, 503 + `Retry-After` |
| G. DB contention on mass reconnect | Bootstraps could take every pool connection | Bootstrap slots = half the pool; pool acquire timeout 10 s |
| Per-connection CPU | Each event was JSON-parsed once per connection | Parsed once per instance |

## 11. CI/CD changes

- `ci.yml` `server-integration`: + lifecycle, + reliability suites; artifacts
  (instance logs, `recovery.jsonl`) uploaded; `timeout-minutes: 30`; Postgres
  and Redis service images pinned by digest.
- New `reliability.yml` (nightly 03:17 UTC + manual): fault-injection suites
  repeated N times with a recovery-window summary, control-plane load profiles
  (configurable), Criterion benches; all artifacts uploaded.
- Docker smoke covers fresh `/run`, non-root processes, `--user 10001`,
  health endpoints, dashboard headers. Supply-chain job fixed by the lockfile.

## 12. Files changed

Server: `src/{sync,metrics,lifecycle}.rs` (new), `src/{main,stream,state,api,client_ip,rate_limit,scheduler}.rs`,
`src/api/flags.rs`, `migrations/20261029000028_archive_keeps_revision.sql`, `Cargo.toml`.
Tests: `tests/{reliability,lifecycle}.rs`, `tests/common/{chaos,cluster,sdk}.rs` (new), `tests/common/mod.rs`.
Bench: `bench/` (new crate), `server/examples/loadgen.rs`, root `Cargo.toml`, `Cargo.lock`.
Container: `Dockerfile`, `docker/entrypoint-full.sh`, `scripts/docker-smoke.sh`.
CI: `.github/workflows/ci.yml`, `.github/workflows/reliability.yml` (new).
Docs: `docs/operations/*` (new), `docs/performance/*` (new, incl. raw results), `docs/self-hosting.md`,
`docs/api-reference.md`, `docs/.vitepress/config.ts`, `CHANGELOG.md`, this report.

## 13. Tests passed, failed, and not run

Passed locally (PostgreSQL 16.15, Redis 7.4.11 in Docker):

- `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features` with `CHECKGATE_REQUIRE_INTEGRATION=1`:
  core 45, server unit 69, flutter 6, RN 6, integration, e2e_sse,
  environment_isolation, rate_limit 5, setup_ownership 9, sdk_health,
  **lifecycle 8, reliability 14** — 0 failures
- Node SDK unit (21), Web SDK (21), RN SDK (20), CLI (19), edge (12), SSR (9)
- Node SDK native e2e over live SSE (fresh database)
- `scripts/docker-smoke.sh` on the rebuilt image (all checks); in-place
  upgrade from the `28eb233` image on the same volumes
- `cargo deny check` (advisories, bans, licenses, sources); `cargo audit`
- Trivy image scan at the release gate (CRITICAL, ignore-unfixed): 0
- Miri: `react-native` 6, `flutter` 6
- React Native Android native: packed SDK builds and links for arm64-v8a,
  armeabi-v7a, x86, x86_64
- wasm32: release build + clippy of the web SDK crate; generated bindings
  evaluated flags correctly in Node
- VitePress docs build (dead-link check)
- Criterion benches; loadgen ×2 + durability check

Not run:

- **Flutter SDK tests** — no Flutter/Dart toolchain on this machine (CI runs them).
- **Browser-hosted wasm tests** — the repo has no `wasm_bindgen_test` suite;
  the wasm module was exercised under Node only.
- **Dashboard tests** — no dashboard file changed.
- Release workflow packaging — not changed beyond the Dockerfile (smoke + Trivy run locally).
- The new `reliability.yml` workflow itself has not run on GitHub yet.

Trivy `config` findings on the Dockerfile: DS-0002 (no `USER`; deliberate —
see §14), DS-0031 (false positive: `SETUP_TOKEN_FILE` is a path), DS-0029
fixed.

## 14. Known remaining risks

- **Image has no `USER` directive.** All-in-one mode must start as root to own
  existing database volumes; every long-running process drops privileges, and
  external mode runs fully unprivileged with `--user 10001:10001`. Scanners
  that require `USER` will still flag the image.
- **Reconciliation cost scales with total flags** (one index scan of all flags
  per instance per interval). Fine at 10⁴ flags; at 10⁵–10⁶ flags with many
  replicas, raise the interval or add an aggregate pre-check.
- **Rate limits are per instance** (documented); N replicas allow N× a client's
  budget when a balancer spreads it.
- **SDK fleets behind one NAT share the SSE-connect budget (120/min)**; a resync
  of ~hundreds of clients behind one address can be throttled into the poll
  fallback. Spread and jitter reduce but do not remove this (§16).
- `TCP_USER_TIMEOUT` is Linux-only; elsewhere a reader that never resumes is
  held until the OS's own TCP timeouts.
- Benchmarks are from one desktop machine; the 5 000-connection profile shares
  CPUs with the generator.

## 15. Compatibility considerations

- SSE contract unchanged for SDKs (event types and fields as before; the
  env-targeted `RESYNC` is internal and never published). REST unchanged;
  new endpoints only. `/health` unchanged.
- New refusals: `503` + `Retry-After` on `/stream` at capacity/busy/draining
  and on `/flags/snapshot` when busy — SDKs already treat these as failures and
  back off.
- Migration 28 changes the revision trigger (archive keeps revision);
  forward-only, safe on existing data.
- Rolling upgrades from pre-revision servers: their events are adopted by
  reconciliation, no resync churn. Mixed old/new instances share the default
  channel name.
- Container: existing all-in-one volumes work (verified upgrade); processes now
  run as `checkgate`/`postgres`/`redis`; the setup-token directory is owned by
  UID 10001. `POSTGRES_*` defaults moved from `ENV` into the entrypoint
  (same values). Running all-in-one with `--user` is refused with a message.
- Static dashboard files now get CSP and other headers; the CSP was checked
  against the built `index.html`.

## 16. Follow-up findings worth addressing

Phase 1 follow-ups evaluated:

1. **Redis-backed distributed rate limiting — deferred.** Moving every
   limiter check to Redis adds a round trip per request and couples request
   admission to Redis availability (fail-open defeats the purpose,
   fail-closed turns a Redis outage into an API outage), contrary to keeping
   PostgreSQL the only hard dependency. The per-instance bound is predictable
   (N × budget) and now documented; a gateway is the better place for a
   cluster-wide limit. Revisit with a sliding-window script + local fallback
   if a single-budget requirement appears.
2. **SDK-key-aware rate limits — deferred, design recorded.** Keying by an
   unverified bearer token lets an attacker mint unlimited budgets (each
   random key → its own bucket → a PostgreSQL auth lookup). Safe design: keep
   a high per-IP ceiling before auth, and a per-SDK-key budget after
   successful auth (keyed by key id). Needs the limiter split around the auth
   middleware.
3. **Non-root container — done** (§5).
4. **Cross-instance SDK health aggregation — deferred, design recorded.** Each
   instance would publish a bounded summary (per-environment counts plus the
   newest N connections) to a Redis hash with a TTL per instance; the health
   endpoint merges live instance entries. Needs per-instance identity and a
   UI change; not a reliability fix.
5. **Static security headers — done** (R5).
6. **RFC 7239 `Forwarded` — done**, opt-in, exclusive with XFF.
7. **Previously unexecuted checks — run** (§13), except Flutter.

Other findings:

- Rollout/multivariate evaluation allocates (hash key via `format!`), despite
  the hashing module's "zero-allocation" comment; a stack buffer would remove
  2–4 allocations per call. Not changed (evaluation engine out of scope).
- Writes commit twice (flag, then audit log in its own transaction); folding
  the audit insert into the flag transaction would halve commit waits and make
  audit atomic with the change.
- SSE bootstrap builds the whole environment in memory; streaming it from a
  cursor would bound memory for very large environments.

## 17. Recommended Phase 3 work

- SDK-key-aware and (if required) cluster-wide rate limiting per §16.
- Cross-instance SDK health view.
- OTLP exporter for traces and metrics.
- Audit-log write inside the flag transaction; allocation-free hashing.
- Long soak (hours) and 10 000+ connection runs on dedicated hardware via the
  new workflow; publish results alongside this baseline.
- (Unchanged from the roadmap: OpenFeature providers, new SDKs, SSO, etc.)

## 18. Draft PR

https://github.com/checkgate-sh/checkgate/pull/22 (draft, base `dev`).
