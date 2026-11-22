# Changelog

All notable changes to Checkgate are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [0.2.0] - Unreleased

Developer adoption: OpenFeature, Python, framework integrations, a local
quickstart, an evaluation playground, SDK diagnostics and an MCP server for AI
agents. Every new SDK surface evaluates with the same Rust engine; no feature
adds a network request per evaluation.

### Upgrading from 0.1.31

- **Two database migrations**, both additive, applied on start:
  `20261121000029_pat_propose_scope` widens the personal-access-token scope
  check to allow `propose`; `20261122000030_change_request_create` adds
  `change_requests.action` (`update` for every existing row) and a unique
  index allowing one pending creation per key. **A 0.1.31 binary will not
  start against a database that has them** (the migrator reports an applied
  migration it does not know), so there is no rolling mixed-version upgrade:
  replace all instances.
- **Creating a flag in an environment that requires approval now needs
  approval** unless the flag is inert (disabled, no `disabled_value`). An
  enabled create there used to apply at once; it now answers `202 Accepted`
  with a pending change request (`action: "create"`) — the same contract as a
  queued PATCH — and the flag exists only after someone else approves it.
  Scripts that POST enabled flags into such an environment and expect `200`
  must handle `202` (the Go client, Terraform provider and Kubernetes operator
  report it as `ErrApprovalRequired`). Environments without approval are
  unchanged.
- **Create no `propose` tokens while any 0.1.31 instance is still serving**:
  0.1.31 does not know the scope and would let such a token create a
  `read_write` token.
- **Rollback:** stop 0.2.0, delete every `propose` token, cancel pending
  flag creations, delete both migrations' rows from `_sqlx_migrations`, start
  0.1.31. Take a database backup before upgrading. Details:
  docs/operations/upgrading-to-0-2-0.md.
- No configuration change is required. The SDK protocol (`/stream`,
  `/flags/snapshot`) is unchanged, so 0.1.31 SDKs work with a 0.2.0 server and
  0.2.0 SDKs with a 0.1.31 server; the new server endpoints are additive.
- Building the Rust workspace now needs Python 3.11+ for the PyO3 crate
  (`PYO3_PYTHON` selects the interpreter).

### Added

- **Core:** `explain()` — the same evaluation as `evaluate_variant`, plus the
  reason (`DISABLED`, `PREREQUISITE_FAILED`, `TARGETING_MATCH`,
  `ROLLOUT_EXCLUDED`, `SPLIT`, `DEFAULT`, `STATIC`), matched rule, rollout
  bucket, weighted variant and prerequisite outcomes. Shared evaluation vectors
  (`sdks/conformance/vectors/evaluation.json`) that every SDK checks.
- **Node.js SDK:** `getDetails()`, `onStatusChange()`, and `getHealth()`
  (diagnostics schema 1, shared with Python).
- **`@checkgate/openfeature-provider`:** OpenFeature server-SDK provider over
  `@checkgate/node`, with reason/variant mapping and READY/STALE/ERROR/FATAL
  lifecycle.
- **Python SDK (`checkgate`):** PyO3 bindings to the engine; SSE, poll
  fallback, revision ordering, offline cache, `get_details()`, `get_health()`;
  FastAPI, Flask and Django examples. Runs the shared fault-injection suite.
- **`@checkgate/node-integrations`:** Express middleware, Fastify plugin and
  NestJS module over one shared client, with request-scoped context and a
  startup timeout policy.
- **Evaluation playground:** `POST /api/environments/{env}/flags/{key}/evaluate`
  and a dashboard page — what an SDK would answer for a user, and why. Read-only.
- **SDK diagnostics on the server:** `last_delivered_revision` per stream,
  `GET /api/health/environments` (convergence per environment), metrics
  `checkgate_snapshot_requests_total` and `checkgate_playground_evaluations_total`.
- **`propose` personal access tokens:** read access plus proposing flag
  changes (always a change request) and withdrawing their own requests; they
  can never create a flag, apply, approve or reject. For automation and AI agents.
- **`@checkgate/mcp-server`:** stdio MCP server for AI agents — read-only by
  default; with `CHECKGATE_MCP_WRITE=1`, change proposals through the normal
  approval workflow (no tool creates, approves or applies anything).
- **Quickstart:** `quickstart/start.sh --seed` (local, development only) with
  Node.js and Python examples; first-run snippets in the setup wizard and an
  empty-environment checklist on the dashboard.

### Security

- **Approval-required environments: flag creation is governed** (inherited
  from 0.1.x, found in the 0.2.0 security review). A project editor could
  create an *enabled* flag directly in an environment that requires approval,
  and SDKs were served it with no review. Such a creation is now a pending
  change request that a different editor or admin must approve; rejection
  leaves no flag. Inert creations still apply at once. Regression tests:
  `server/tests/governance.rs`.

### Changed

- Node.js SDK: on a stream's `ready` the client now marks itself ready before
  switching to `streaming`, so status listeners see `isReady()` on a first
  connection.
- CI tests the Node.js SDK's native build, the OpenFeature provider and the
  framework integrations on Node.js 22, 24 and 26 (Node.js 20 reached end of
  life on 2026-04-30); the MCP server on 22.
- Packages not yet on a registry: the Python package (`checkgate` on PyPI) has
  no publish job yet; `@checkgate/openfeature-provider`,
  `@checkgate/node-integrations` and `@checkgate/mcp-server` publish with the
  release (packaging checked in the release workflow).

## [0.1.31] - 2026-11-21

### Upgrading from 0.1.30

No database migration or configuration change is required, and rolling back to
0.1.30 needs no database step (tested: upgrade, mixed versions and rollback on
one database). SDKs and servers of either version work together. Check these
first; the first one deletes data unless you configure it:

- **Impressions and goal events older than 90 days are now deleted.** 0.1.30
  kept them forever. A retention job removes rows received more than
  `ANALYTICS_RETENTION_DAYS` (default 90) ago, except those of running or
  paused experiments, starting about a minute after the server starts and in
  small batches. **Set `ANALYTICS_RETENTION_DAYS=0` before upgrading to keep
  everything.** Results of completed experiments older than the retention
  shrink as their rows expire. Deleted rows are not restored by a rollback.
- **Analytics ingestion has a quota per SDK key** (and per user), shared by
  all instances through Redis: 20,000 requests and 60,000 rows a minute by
  default (`RATE_LIMIT_INGEST_KEY_PER_MINUTE`,
  `RATE_LIMIT_INGEST_ROWS_PER_MINUTE`). Over it, the batch gets `429` with
  `Retry-After` and nothing is stored; the SDKs drop it (no retry). Raise the
  row budget first if one key reports more than about 1,000 rows a second.
- **SDK `connect()` can now reject** (it used to stay pending forever): with
  `CheckgateAuthError` (Flutter: `CheckgateAuthException`) when the server
  answers 401/403, and when `disconnect()`/`close()` is called first. A caller
  that never awaits `connect()` gets no unhandled rejection. After a 401/403
  the SDK stops retrying instead of reconnecting every 30 s forever; flags
  already loaded keep being served, and calling `connect()` again retries.
- **SSE keep-alive is now `event: heartbeat`** (with `data: {}`) instead of an
  SSE comment, sent after `SSE_HEARTBEAT_INTERVAL_SECS` (default 15) of
  silence, and `connected` carries `heartbeat_ms`. Older SDKs ignore both.
  Proxies see the same traffic pattern as before.
- **`GET /flags/snapshot?v=2`** answers `{ v, flags, revisions, tombstones }`;
  0.1.31 SDKs ask for it when they poll. Without `v` the endpoint answers the
  same array as before, so older SDKs and other consumers (`@checkgate/edge`,
  `@checkgate/ssr`, scripts) are unaffected. A 0.1.31 SDK polling a 0.1.30
  server gets the array and behaves as 0.1.30 did (no ordering between a poll
  and a stream replay; see R20 below).
- **SDKs drop a silent stream** after 3 heartbeat intervals (45 s by default)
  and reconnect. Against a 0.1.30 server the JavaScript SDKs keep only the new
  30 s connect deadline (that server's keep-alive is invisible to
  `EventSource`); Flutter still sees it.

- **Configuration limits.** Flag and segment writes past these limits now get
  `422`: more than 500 rules on a flag; the same segment referenced twice;
  more than 50 distinct segments on a flag; a prerequisite on the flag itself,
  listed twice, or more than 50; more than 100 variants; more than 500 rules
  in a segment. Stored flags keep working. The upgrade guide
  (`docs/operations/upgrading.md`) has a query that finds flags a later edit
  would reject.
- **`GET /flags/snapshot` answers `500` when a stored flag does not decode**,
  instead of silently leaving it out (which polling SDKs treated as a delete).
- **Tokens.** A personal access token can no longer create a token that
  outlives itself, and a read-only token can no longer revoke tokens.
- **Webhooks and integrations no longer follow redirects**, and store at most
  1 KiB of each response.
- **Webhook and integration URLs may not point at link-local addresses**
  (`169.254.0.0/16`, where cloud metadata services answer, and `fe80::/10`),
  at `fd00:ec2::254` or at the unspecified address (`0.0.0.0/8`, `::`):
  creating or changing one answers `422`, and a hostname that resolves only
  to such addresses fails to connect. Rows stored earlier are refused at
  delivery and logged as such. Loopback and private addresses still work.
- **Integration test messages** (`POST .../integrations/{id}/test`) answer
  `200` with `{ delivered, status_code, error }` after one attempt (at most
  10 s), instead of `202` before anything was sent. They now reach only the
  tested integration, also when it is disabled or filtered.
- **Flag reads with an SDK key** (`GET /api/environments/{env}/flags[/key]`)
  no longer include `owner_email`. The field was already omitted for flags
  without an owner. Tags are unchanged. Sessions and personal access tokens
  still get it.
- **Revoking an SDK key closes the open streams of its environment**, on
  every instance, within `SSE_RESYNC_SPREAD_MS` (default 5 s). Clients using
  other keys reconnect and re-bootstrap once; the revoked key's reconnect is
  refused. New metric label: `checkgate_resync_requests_total{trigger="key_revoked"}`.
- **Logs.** The request span records `path` (no query string) instead of
  `uri`, so `?sdk_key=` is no longer logged. Webhook logs no longer include
  the URL.
- **All-in-one container.** The entrypoint shell stays as PID 1 and stops the
  bundled PostgreSQL and Redis after the server exits. Keep the stop timeout
  at 30 s or more (`docker-compose.yml` now sets `stop_grace_period: 30s`).

### Known limitations

- **Flutter**: the package bundles no compiled engine; link the Rust engine
  as the Flutter README describes (verified on an iOS simulator).
- **iOS**: verified only for Flutter, on the iOS 17 simulator. The React
  Native iOS app build and physical devices were not tested for this release.
- **Ordering across poll and stream** (the R20 fix) needs a 0.1.31 server;
  against an older server the SDKs behave as 0.1.30 did. It is per flag, not
  across flags (`docs/operations/configuration-consistency.md`).
- **Propagation tail**: the 1 000-client p99 measured higher than 0.1.30's on
  a contended macOS host (198 against 75 ms pooled; median run 84 against
  76 ms). Re-measured on a Linux desktop (native Docker, alternated runs, 8
  per version), it does not reproduce: p50 and p99 are statistically equal at
  1 to 1 000 clients (1 000-client p99 0.82× of 0.1.30's, 95 % CI 0.72–1.09),
  none of 5.1 million deliveries missed. With PostgreSQL's commit latency
  removed, the 1 000-client p99 is 1.8 ms (6 %) higher, all of it in the first
  20 updates after the clients connect (a one-time warm-up cost; later updates
  are equal). Not a dedicated server, and its disk dominates the absolute
  numbers.
- **Security findings not fully closed** (all low or informational):
  webhook and integration URLs may still target loopback and private
  addresses by design (restrict egress at the network layer if they must
  not); removing a project member does not yet close that user's open
  dashboard stream until it reconnects (revoking an SDK key does); anyone who
  knows an account's email can still lock it for 15 minutes with 5 wrong
  passwords; SDK keys, which are public by design, are stored unhashed.

### Added

- **Analytics retention** (`ANALYTICS_RETENTION_DAYS`, default 90; `0` keeps
  rows forever; `ANALYTICS_RETENTION_INTERVAL_SECS`, default 3600;
  `ANALYTICS_RETENTION_BATCH_SIZE`, default 5000). One instance at a time runs
  a pass (PostgreSQL advisory lock), at most once per interval across the
  deployment, deleting in short batched transactions on the existing
  `received_at` indexes. Rows of running or paused experiments are kept. No
  migration.
- **Per-key ingest quota**, atomic in Redis across instances; when Redis does
  not answer, each instance enforces the same budget alone. Large bodies are
  charged by size (one row per KiB), so a few huge rows cost what they store.
- Metrics: `checkgate_analytics_rows_total{table,outcome}`,
  `checkgate_ingest_quota_rejections_total{limit}`,
  `checkgate_ingest_quota_local_decisions_total`,
  `checkgate_analytics_cleanup_runs_total{outcome}`,
  `checkgate_analytics_cleanup_deleted_rows_total{table}`,
  `checkgate_analytics_cleanup_duration_seconds`,
  `checkgate_analytics_cleanup_last_success_timestamp_seconds`.
- **Dead-stream detection in every SDK** (`connectTimeoutMs`, 30 s, and
  `streamIdleTimeoutMs`, default 3 × the announced heartbeat; Flutter
  `connectTimeout`, `streamIdleTimeout`). A dropped NAT mapping or half-open
  connection used to leave an SDK on stale flags until the OS gave up (hours).
- **`getStatus()`** (Flutter: `status`): `state` (`connecting`, `streaming`,
  `reconnecting`, `polling`, `auth_failed`, `closed`), `lastSyncAt` (heartbeats
  on a live stream count as confirmation) and `error`, so an app can tell how
  fresh its flags are.
- **`SSE_HEARTBEAT_INTERVAL_SECS`** (1–300, default 15).
- **SDK fault-injection suite** (`sdks/conformance`): one randomized, seeded
  suite every SDK runs against its real transport; nightly over 300 seeds.
  **Live end-to-end runs of all four SDKs** against a real server
  (`scripts/sdk-live-e2e.sh`, in CI). Server-side randomized fault and segment
  read failure tests (`server/tests/sse_faults.rs`).
- A workflow that submits the resolved Go module graph, so Dependabot stops
  reporting the already-patched `google.golang.org/grpc` (#51, #54, #55).
- Prometheus process metrics on `/metrics` (Linux): `process_cpu_seconds_total`,
  `process_resident_memory_bytes`, `process_virtual_memory_bytes`,
  `process_open_fds`, `process_threads`, `process_start_time_seconds`.
- `bench/ab/`: scripts to compare two builds (Node evaluation, snapshot
  throughput and CPU, a Node differential check, upgrade and rollback).

### Security

- **One editor could exhaust memory for the whole server** by referencing a
  segment thousands of times in a flag: each reference copied the segment's
  rules, for every publish, SSE bootstrap and snapshot. Duplicate references
  are rejected and expansion is bounded (see the limits above).
- **Prerequisite fan-out could freeze every SDK.** Prerequisites that are all
  satisfied never short-circuit, so a chain of flags that each list the next
  one several times cost N^depth evaluations per `isEnabled`. The core now
  caps prerequisite evaluations at 1 000 per evaluation (failing closed), and
  the server rejects self-references, duplicates and more than 50.
- **Webhook SSRF through redirects.** The delivery client followed redirects
  to any host and stored the full response, readable by project viewers.
  Redirects are no longer followed and bodies are capped.
- **An integration's URL (its posting credential) leaked through delivery
  errors** stored for viewers and written to logs. It is now removed.
- **Unauthenticated logins could stall an instance.** Argon2 ran on async
  worker threads; it now runs on the blocking pool, at most one per CPU.
- **`?sdk_key=` was written to the server's own logs** through the request span.
- Read-only tokens could revoke the user's other tokens, and any token could
  mint a non-expiring one. Audit-log paging was unbounded (a negative limit
  gave a 500).
- `quinn-udp` 0.5.14 → 0.5.16 (RUSTSEC-2026-0538). Only in the lockfile; never
  compiled.
- **Concurrent logins could test more passwords than the lockout allows.** The
  failure count was checked before the password and recorded after it, so a
  burst of parallel attempts on one account all passed the check (20 of 20 in
  a test against a limit of 5). Each attempt is now recorded before its
  password is checked, so at most 5 are checked per 10-minute window.
- **Webhook and integration URLs could target cloud metadata services
  directly** (`169.254.169.254`, `fd00:ec2::254`). Link-local and unspecified
  destinations are refused when the URL is saved and when it is resolved.
- **Flag reads with a public SDK key returned the flag owner's email
  address.** It is now omitted for SDK keys.
- **A revoked SDK key's open streams kept receiving updates** until they
  reconnected. Revocation now closes them on every instance.
- **Analytics storage could grow without bound** (N6). SDK keys are public,
  ingestion was limited only per client address (600 requests of up to 500
  rows a minute, so 300,000 rows a minute from each address), and nothing was
  ever deleted. In a test, 40 concurrent 500-row batches with one key across
  two instances were all stored. Ingestion is now also limited per key across
  instances (with a 5,000-row budget the same test stores 10 or 11: the burst
  plus what refills while it runs), and rows expire after the retention
  period.

### Fixed

- **Integration "Send test message" did not test the selected integration.**
  It queued a real `flag.updated` event for the environment, so it posted to
  every enabled integration subscribed to that event, sent nothing to a
  disabled or filtered one, and showed success (`202`) before anything was
  sent, including when the channel rejected the message or could not be
  reached. It now posts to that integration only and the dashboard shows the
  channel's answer or the reason it failed. A malformed id answered `500`; it
  is now `404`.
- **A delayed stream replay could roll flags back (R20, every SDK, also in
  0.1.30).** While an SDK polled during a stream outage, the next stream's
  replay (read by the server when the stream opened) could arrive after polls
  had delivered newer state; on `ready` the SDK swapped the older replay in,
  so flags went back, and a flag deleted meanwhile came back, until the live
  events behind the replay caught up. Polls carried no revisions, so the SDK
  could not tell which was newer. The snapshot now carries them (`?v=2`), and
  every SDK merges a complete flag set (replay or poll) per flag with what it
  learned after requesting it: the newer version wins, and a set still
  replaces everything older. Guarantee and limits:
  `docs/operations/configuration-consistency.md`.
- **A duplicate change event notified `onChange` listeners again**, and a
  delete of a flag the SDK did not hold notified them too. Both now change
  nothing and notify nobody.
- **React Native: after a network error the SDK stayed on stale flags until
  the app restarted.** `react-native-sse` (the polyfill the docs require)
  supports `addEventListener('error')` but not `onerror`, so the SDK never saw
  a stream failure, and the polyfill does not reconnect after one. The SDK now
  listens properly, closes replaced streams for good (the polyfill re-opened a
  stream closed during its own error dispatch) and skips the polyfill's 500 ms
  delay before each connection.
- **Flutter: the package could not be imported.** Its library was outside
  `lib/` in every published version. It also bundles no compiled engine; the
  docs now describe how to link one (verified on an iOS simulator).
- **SDKs retried a rejected key forever** and `connect()` never settled.
- **Web: `disconnect()` during WASM start-up leaked a stream**, and a
  `disconnect()` + `connect()` without storage served `false` for every flag
  the replay found unchanged (a fresh engine was created each time).
- **Web: the TypeScript declarations did not describe `CheckgateWeb`** (the
  package pointed `types` at the WebAssembly bindings), so the documented
  TypeScript usage did not compile. The package now ships `index.d.ts` for
  the client, its options, `getStatus()` and `CheckgateAuthError`.
- **Flutter: `close()` followed quickly by `connect()` left two SSE loops.**
- **Web, React Native and Flutter SDKs: one non-string attribute erased every
  attribute**, so a user excluded by a `not_equals` rule was let through. Each
  invalid attribute is now skipped on its own.
- **Flutter: large bootstraps lost flags, and `connect()` could hang.** The
  SSE parser split each network chunk on newlines without carrying partial
  lines over, so lines cut across socket reads were dropped, sometimes
  including the `ready` event.
- **Flutter: a slow poll could roll back newer stream state** after a
  reconnect. Polls are now single-flight, time out after 30 s, and discard
  responses that arrive after the stream takes over.
- **React Native: without an `EventSource` polyfill, `connect()` never
  settled.** React Native ships none. The SDK now logs what to install and
  polls meanwhile; the docs require `react-native-sse`.
- **A flag change whose segments could not be read was published with its
  segment rules stripped**, and reconciliation could not repair it. Such a
  change now re-bootstraps the environment's clients instead
  (`checkgate_resync_requests_total{trigger="segment_read_failed"}`).
- `/flags/snapshot` read flags and segments in two statements (a segment edit
  in between gave a mixed snapshot) and silently dropped undecodable flags.
- **All-in-one container: PostgreSQL and Redis were SIGKILLed on every
  `docker stop`**, so PostgreSQL ran crash recovery on each start. They are
  now shut down cleanly. `scripts/docker-smoke.sh` checks this, and also runs
  on macOS's bash 3.2.

### Performance

- **Node SDK evaluation is 1.9–8.8× faster** (boolean flag: 6.9 µs → 0.78 µs,
  5 attributes). The binding reads only the attributes the flag's rules
  reference, instead of converting the whole object on every call. Results
  are identical (360 000-call differential check against 0.1.30). An attribute
  that no rule reads is no longer validated, and `null` attributes no longer
  throw.
- **`/flags/snapshot` and SSE bootstrap use about a third less CPU per
  request**: flags are decoded straight from `jsonb`, without an intermediate
  JSON tree, and the snapshot is one query. An isolated A/B at 10 concurrent
  clients measured −33 % server CPU per request and +44 % throughput; the
  public benchmark (1 vCPU server) shows −11 % to −21 % CPU per request at 100
  to 1,000 clients.

### Documentation

- Performance claims now match measurements: "sub-microsecond" only for the
  Rust core, "a few microseconds" for the Node SDK, "tens of milliseconds" for
  propagation. Unsourced numbers about other products were removed.
- The evaluation diagram includes prerequisites.
- New pages: [Upgrading to 0.1.31](docs/operations/upgrading.md) and
  [Configuration consistency](docs/operations/configuration-consistency.md).
- Internal release-engineering reports are no longer kept in the repository's
  `docs/` tree (they remain in Git history); the docs build fails if internal
  files would be published.
- Self-hosting, API reference, observability and upgrade pages document
  analytics retention, the per-key ingest quota and its `429`, the ingest
  error responses, how to size both, and SQL to inspect analytics storage.
- The API reference covers the Slack/Teams integration routes, flag
  archive/unarchive and `GET /api/projects/{id}`. The docs site now installs
  from its pnpm lockfile (`--frozen-lockfile`), so every build uses the same
  dependency versions.

## [0.1.30] - 2026-11-11

Versions 0.1.26–0.1.29 were skipped; 0.1.30 directly follows 0.1.25.

### Upgrading from 0.1.25

Nothing needs to change for most deployments. Check these before upgrading:

- **CLI scripts that test for exit code `1` exactly.** Usage errors now exit `2`, and `--check`
  drift exits `3` (see Changed). Checks for non-zero are unaffected.
- **More than 10 000 SSE connections on one instance.** `SSE_MAX_CONNECTIONS` now caps
  connections per instance (default 10 000; there was no limit). Further connections get `503`
  with `Retry-After`. Raise the limit or add instances.
- **Running the image with a fixed user.** The server runs as UID 10001. External mode accepts
  `--user 10001:10001` / `runAsNonRoot`. All-in-one mode must start as root (its entrypoint drops
  privileges) and refuses to run as any other user.
- **Slow database connections.** The PostgreSQL pool now waits at most 10 s for a connection
  (was 30 s).
- **Shutdown timing.** SIGTERM drains and exits within `SHUTDOWN_GRACE_PERIOD_SECS` (default 20).
  Keep your orchestrator's kill timeout above that (Kubernetes' default is 30 s).

No server API, SSE wire format or SDK API changed incompatibly. One migration
(`20261029000028_archive_keeps_revision`) runs automatically at startup. It only replaces the
`flags_bump_revision()` trigger function, with no table rewrite. Running 0.1.25 servers keep
working during a rolling upgrade. However, a 0.1.25 server that **starts** against a migrated
database refuses to boot, because it does not know the newer migration. This affects rollback
and any old instance restarted mid-rollout. 0.1.25's own migration behaved the same way.

### Fixed

- **CLI: generated code that did not compile.** A string default containing a carriage return
  broke the Rust output (bare CR is illegal in a raw string) and the Dart output (raw CR in a
  one-line string); a flag keyed `self` became the Rust keyword `Self`; Dart flags named
  `values`, `index`, `hashCode`, `toString`, `runtimeType`, `noSuchMethod`, `dynamic`, `bool`,
  `int` or `key` clashed with reserved or generated names; de-duplicated Rust variants (`AB_2`)
  failed crates built with `-D warnings`. CI now compiles the generated code with `rustc`,
  `tsc --strict` and `dart analyze` instead of only generating it.
- **CLI: a flag default that does not match its type is an error naming the flag**, instead of
  silently producing code that does not compile (a string on an integer flag), `NaN`, or a
  constant that differs from the server's value (an integer beyond ±2^53, a fraction truncated).
  A `null` default on a non-`json` flag counts as no default.
- **CLI: a server that never answers no longer hangs the run.** Requests time out after
  `--timeout` seconds (default 30, `CHECKGATE_TIMEOUT`).
- **CLI: credentials in `--url` were printed to the terminal.** `fetch` refuses such URLs and its
  error repeated the URL, so `https://user:password@host` landed in CI logs. Such a URL is now
  refused up front without echoing it (use `--token`).
- CLI: `--version` printed `0.1.20` regardless of the installed version; it now reads
  `package.json`.
- CLI: an HTML response (dashboard URL, proxy or SSO page) is reported as such instead of
  `Unexpected token '<'`, and a server error includes the server's message.
- CLI: output files are written atomically (temp file + rename), so Ctrl+C or a kill never leaves
  a truncated file; an identical file is not rewritten (its mtime does not change).
- **A change committed by a server that died before publishing it now reaches every client.**
  Redis Pub/Sub delivers at most once, and no server held the lost event, so clients of the
  other servers stayed on the old configuration until they reconnected for another reason.
  Each server now reconciles the revisions it has relayed against PostgreSQL every
  `SYNC_RECONCILE_INTERVAL_SECS` (default 30, jittered) and re-bootstraps only the clients of
  an environment whose changes were lost. This also repairs changes made directly in SQL.
- A Redis subscription that stays open but stops delivering (lost NAT or load-balancer state) is
  now detected by a ping every `REDIS_PING_INTERVAL_SECS` (default 10) and reconnected, with a
  resync. Previously the server stopped relaying changes indefinitely without noticing.
- Graceful shutdown no longer hangs while SSE streams are open. SIGTERM now fails readiness,
  waits `SHUTDOWN_DRAIN_DELAY_SECS`, closes every stream, stops background tasks and exits within
  `SHUTDOWN_GRACE_PERIOD_SECS` (default 20); previously every shutdown with a connected SDK ended
  in SIGKILL.
- A client that stops reading its stream no longer holds the connection and its resources
  forever: connections whose data stays unacknowledged for `TCP_USER_TIMEOUT_SECS` (default 60,
  Linux) are aborted.
- Dashboard files now carry the security headers (CSP, `X-Frame-Options`, `nosniff`, …); they
  were served outside the middleware stack. The dashboard's CSP allows its web fonts.
- Archiving or unarchiving a flag no longer assigns it a new revision (it changes nothing SDKs
  evaluate and publishes no event).
- The bundled PostgreSQL starts when the container's `/run` is a fresh tmpfs (some runtimes and
  BuildKit builders), which broke all-in-one mode in CI.

### Added

- CLI `--check`: compares generated output with the files on disk, writes nothing, and exits `3`
  when a file is missing or stale. The `// Source:` header is ignored, so output generated from
  the server checks clean against an export of the same flags.
- CLI `--json`: one machine-readable result object on stdout (`schema_version: 1`): status, exit
  code, per-file status, findings with an `action`, and a classified `error.code`. See
  `docs/ecosystem/cli.md` for the schema, a GitHub Actions example and the agent contract.
- CLI `--timeout` / `CHECKGATE_TIMEOUT`.
- Prometheus/OpenMetrics metrics on a separate listener (`METRICS_ADDR`, off by default):
  propagation, publish failures, Redis subscription state, resyncs, reconciliation, SSE
  connections and rejections, bootstrap and snapshot latency, database pool and readiness.
  See `docs/operations/observability.md`.
- `GET /health/live` (never fails on a dependency outage) and `GET /health/ready` (PostgreSQL
  required; Redis optional via `READY_REQUIRES_REDIS`; fails while draining). `GET /health` is
  unchanged. The image's `HEALTHCHECK` uses `/health/live`.
- Admission control for SSE: `SSE_MAX_CONNECTIONS` per instance (default 10 000) and
  `SSE_BOOTSTRAP_CONCURRENCY` database slots shared by bootstraps and snapshot polls; refusals
  answer `503` with a jittered `Retry-After`. Resynced streams close over
  `SSE_RESYNC_SPREAD_MS` (default 5 s) instead of all at once, and only the affected
  environment's streams are resynced after reconciliation.
- `TRUSTED_PROXY_HEADER=forwarded` reads the RFC 7239 `Forwarded` header from trusted proxies
  instead of `X-Forwarded-For` (never both).
- `REDIS_CHANNEL` selects the Pub/Sub channel, so separate deployments can share a Redis.
- Multi-instance fault-injection suite (`server/tests/reliability.rs`) and lifecycle suite
  (`server/tests/lifecycle.rs`), run in CI; a nightly/on-demand workflow repeats them and runs
  load tests and benchmarks. Criterion benchmarks for the evaluation core and a control-plane
  load generator (`server/examples/loadgen.rs`); methodology and baseline in `docs/performance/`.

### Changed

- **CLI exit codes:** usage errors (unknown, missing or conflicting options) now exit `2` instead
  of `1`; `--check` drift exits `3`; operational failures stay `1`; Ctrl+C / SIGTERM exit `130` /
  `143`. Running `checkgate` with no command prints help to stderr and exits `2` (was stdout,
  `1`). Scripts that only test for non-zero are unaffected.
- **CLI: unknown options are rejected** (with a "did you mean"), as are `--out` together with
  `--out-dir`, `--input` together with `--url`/`--env`, and an option missing its value.
  Previously a typo such as `--out-dirr` was ignored and the code went to stdout with exit `0`.
- **Faster rollout and multivariate evaluation in every SDK built on `checkgate-core`**: the bucketing
  hash is now computed over the key parts directly instead of a `format!`ed string. Same bytes,
  same buckets (tested over every split of inputs up to 40 bytes); allocations per evaluation 2 → 0
  (rollout) and 4 → 1 (multivariate); Criterion time −57 % and −59 % on the same machine.
- CI workflow runs with `permissions: contents: read`.
- The Docker image runs no process as root. The server runs as `checkgate` (UID 10001); in
  all-in-one mode the bundled PostgreSQL and Redis run as their own users, and the entrypoint
  only starts as root to prepare their volumes. External mode supports `--user 10001:10001` /
  Kubernetes `runAsNonRoot`. Running all-in-one mode as a non-root user is refused with a clear
  message.
- Dashboard files and unknown paths are no longer charged to the API rate-limit budget; they
  were previously served outside rate limiting entirely, which this preserves.
- The PostgreSQL pool waits at most 10 s for a connection (was sqlx's 30 s default).

### Dependencies

- rustls-webpki 0.103.16 (RUSTSEC-2026-0382), webpki-roots 1.0.11 (RUSTSEC-2026-0381),
  tokio 1.54.1 (RUSTSEC-2026-0386).

## [0.1.25] - 2026-10-19

### Security

- **SDK keys are no longer admin credentials.** They ship inside browser and mobile apps, yet
  every API route accepted them as a workspace admin. An SDK key now only reaches the SDK
  endpoints — `/stream`, `/flags/snapshot`, reading flags, and reporting impressions and events —
  and only for the environment it is bound to. **Breaking:** automation that used an SDK key
  (Terraform, the Kubernetes operator, scripts) must switch to a `read_write` personal access token.
- Impression and event ingest now authorize the caller: an SDK key for that environment, or a
  user with write access to it. Previously any authenticated caller, including a viewer from
  another project or a `read_only` token, could write analytics to any environment.

### Dependencies

- Patched Dependabot and RustSec advisories: rustls 0.23.46, quinn-proto 0.11.19, xxhash-rust
  0.8.19 (plus the yanked chacha20 and spin releases); vitest 4.1.12, postcss 8.5.30,
  source-map-js 1.2.2, js-yaml 4.3.2; grpc-go 1.83.1 in the Terraform provider. The React
  Native SDK no longer installs its `react-native` peer dependency for local development, which
  removes that tree's advisories from its lockfile.

### Fixed

- A broken Redis publishing connection is replaced automatically instead of failing every later
  write's SSE update. If a publish still fails, the update reaches this server's SSE clients
  directly and the other servers' clients are resynced once Redis is back; servers also resync
  their clients after re-establishing a dropped Redis subscription.
- Flag change events carry a `revision`, and all SDKs ignore events older than the flag state they
  hold, so concurrent writes can no longer leave clients on stale configuration. Segment edits give
  every flag using the segment a new revision, and the SSE bootstrap replays recent deletions, so
  a delayed event can't restore a deleted flag on a client that connected after the delete.
- SDK change listeners fire only after a resync snapshot is fully applied, so a listener never
  evaluates against a half-updated flag set.
- SDKs keep evaluating the last complete flag set during a reconnect, swapping in the replay only
  once it is complete. Previously `isReady()` stayed `true` over an emptied store, and a replay cut
  short left a partial flag set live.
- SDKs timestamp impressions (`evaluated_at`) and conversions (`occurred_at`) when they happen,
  so a conversion batch that reaches the server first is no longer dropped from experiment results.
- Changing a project's default environment no longer fails with HTTP 500, and each project can have
  its own default (the uniqueness index was global).
- The dashboard and `checkgate typegen` fetch every page of the flag list instead of stopping at 200.
- The Go client gains `ReplaceFlag`, which clears fields that are empty in the desired state;
  Terraform and the Kubernetes operator use it, so removing rules, prerequisites, variants, tags,
  or a description from configuration now clears them on the server.
- Segment PATCH accepts `"description": null` to clear the description, and the Go client gains
  `ReplaceSegment`; Terraform uses it, so a description or rules removed from configuration are
  cleared on the server.
- Environment slugs are unique per project instead of across all projects, so a second project
  can have its own `production`.
- `@checkgate/react-native` now builds and links its Rust engine (Gradle via `cargo ndk`, CocoaPods
  via a build phase) and ships the native module that installs its JSI bindings. Building the app
  needs a Rust toolchain; see the package README.

## [0.1.24] -  2026-09-17

### Added

- A dashboard screenshot guide and a Playwright capture script backed by a disposable Docker demo,
  using the Vantage Robotics workspace and administrator Juan Dela Cruz.
- Scheduled changes expose `attempts` and `last_error` for diagnosing failed executions.
- Flag snapshots include an `X-Checkgate-Environment-Id` header, exposed through CORS, so SDKs
  can report analytics when using the polling fallback.
- Workspace-wide Clippy safety policy (`unwrap`/`panic`/indexing/arithmetic/casts) enforced at
  `deny`, with test code exempted; `#![forbid(unsafe_code)]` on every crate except the two FFI SDKs.
- `cargo-deny` and `cargo-audit` configuration, plus CI jobs for supply chain, Miri, and docs.
- Toolchain pinned via `rust-toolchain.toml`.
- `// SAFETY:` documentation on all 19 unsafe blocks in the React Native and Flutter FFI crates,
  backed by pointer-level smoke tests that run under Miri (no UB found).

### Changed

- Refreshed all eight README and documentation dashboard screenshots from the Docker demo,
  showing indigo branding, the gate logo, and administrator Juan Dela Cruz.
- Dashboard tooling, CI, and the Docker dashboard builder now use Bun 1.4.2. The Docker server
  builder uses Rust 1.98 to match the pinned workspace toolchain.
- Session cookies now include a user ID and a server-enforced seven-day expiry. Existing sessions
  require a fresh login after upgrading.
- Node.js, Web, and React Native persisted flag caches are scoped to both the server URL and a hash
  of the SDK credential. Old cache entries are ignored after upgrading.
- A new database migration adds scheduled-change retry tracking and runs automatically at startup.
- **New brand.** Indigo `#4F46E5` is used throughout dashboard navigation, actions, enabled
  controls, and success indicators. Gray marks inactive state; red and amber communicate errors
  and pending reviews. The default Development environment color also changes to indigo.
  New minimalist logo and a banner replacing the 1.7 MB PNG duplicated into four directories.
- GitHub organisation moved from `checkgate-dev` to **`checkgate-sh`**. Go module paths, npm
  `package.json` URLs, the podspec, pubspec, docs, and `ghcr.io` image paths all follow.
  Consumers of the Go client, operator, or Terraform provider must update their import paths.
- Variant colors on the Exposure page exclude indigo to keep variants distinct from brand
  controls and enabled state.
- Variant bucketing carries its total weight as `NonZeroU64`, making the zero-weight case
  unrepresentable rather than guarded.
- `checkgate_is_enabled_ctx` now accepts a NULL context and fails closed instead of dereferencing it.

### Fixed

- Removed the remaining green styles from dashboard toggles, status badges, and success indicators.
  Change requests and environment comparison now display their page titles in the top bar.
- SDK-key revocation and project deletion no longer fail because of invalid PostgreSQL aggregate
  locking queries; safeguards for the last key and project remain in place.
- Change-request approval applies the flag update and approval status in one transaction. Failed
  updates leave the request pending so approval can be retried.
- Scheduled changes retain their row lock through execution, preventing duplicate application across
  replicas. Failed updates roll back and remain pending, with a 60-second retry backoff.
- Scheduled patches are validated before they are stored, including patch types and execution times.
- Node.js, Web, and React Native polling deduplicates concurrent requests and discards stale responses
  after SSE takes over or the client disconnects. Polling-only bootstrap now enables analytics.
- Variant weights whose total exceeds the 32-bit hash range now use the full weighted distribution;
  assignments for smaller totals remain unchanged.
- Flag list pagination accepted an unbounded `limit`/`offset`; large values could also wrap to a
  negative SQL `LIMIT`. Both are now clamped.
- The server integration and SSE suites reported success while skipping when their database
  environment variables were unset. They still skip locally but now fail hard under CI.
- Stale doc comment in the hashing core, and two unresolved rustdoc links.
- The dashboard's `brand` color ramp was two different hues stitched together — steps 50–400 were
  Tailwind green, 500–950 emerald — so tints never matched the primary. It is now one hue, and is
  actually referenced: every component previously hardcoded `emerald-*` and the token was unused.

### Security

- Session and personal-access-token SSE connections only receive bootstrap flags and live updates
  from authorized projects.
- Environment writes enforce project membership roles, preventing project viewers from writing
  through a broader workspace editor role while allowing project editors to edit their projects.
- Sessions validate the current user account and role on each request; expired cookies and sessions
  belonging to deleted users are rejected. SDK credentials are checked against the database so
  creation and revocation take effect across replicas.
- Approval-protected environments reject flag replacement through POST, deletion, promotion,
  scheduling, and segment mutations with HTTP 409. New flag creation remains allowed; existing flag
  edits must use PATCH and the change-request review flow. Previously scheduled changes wait if
  approval protection is enabled before execution.
- `rustls` upgraded to 0.23.45 (RUSTSEC-2026-0285). The outstanding `rsa` advisory
  (RUSTSEC-2023-0071) is lockfile-only via `sqlx-mysql` and never built; suppressed with rationale.

## [0.1.23] - 2026-08-02

- Updated `quinn-proto` for a published CVE.
- Documentation and contribution-workflow updates.

## [0.1.22] - 2026-07-18

### Changed — License

- **Relicensed from MIT to Apache License 2.0.** Both are permissive; Apache 2.0 adds an explicit
  patent grant, a patent-retaliation clause, and an explicit trademark disclaimer. A `NOTICE` file
  was added. **Releases up to and including `v0.1.21` remain MIT-licensed** — that grant is
  irrevocable for anyone who already received them.

### Added

- **Slack & Microsoft Teams alerts**, configured per environment, with native Block Kit and
  MessageCard formatting. Seven subscribable event types covering flag and change-request activity —
  change-request events are new to the notification system entirely, and reach raw webhooks too.
  Delivery reuses the webhook pipeline (retries, bounded delivery log, test-message action); the
  incoming URL is treated as a credential and never returned after creation.

### Changed

- Outbound events fan out through a single `notify()` entry point, so a new event cannot reach one
  sink and silently miss another.
- Dashboard builds and runs on [Bun](https://bun.sh) instead of pnpm + Node. Contributors need Bun
  1.3+ for `dashboard/`; use `bun run test`, not `bun test`.
- Flag create/edit moved into a URL-addressable slide-over panel; old routes redirect.
- Sidebar navigation grouped by scope (Environment, Project, Workspace), cutting the default list
  from fifteen items to ten.

### Fixed

- Architecture-diagram caption overflowed the SVG viewBox.

## [0.1.21] - 2026-07-18

- Documentation and release-workflow corrections. No library, SDK, or server changes.

## [0.1.20] - 2026-07-18

### Added

- **SSR / bootstrap helpers** (`@checkgate/ssr`) — server-render initial flag state and hydrate with
  zero flicker, via an XSS-safe embedded snapshot. Works with any SSR framework; zero dependencies.
- **Edge Side Evaluation** (`@checkgate/edge`) — runtime-agnostic edge evaluator with TTL and
  stale-while-revalidate caching, fail-open on origin outages, delegating to the shared WASM engine
  so results match every other SDK. Ships Cloudflare Workers and Fly.io recipes.
- **Infrastructure as code** on a shared Go API client — a Terraform/OpenTofu provider (flags,
  segments, import, approval-aware) and a Kubernetes operator reconciling a `FeatureFlag` CRD with
  drift correction and finalizers.
- **Type-safe schema CLI** (`@checkgate/cli`) — `checkgate typegen` generates flag accessors for
  TypeScript, Dart, and Rust from a live server or a JSON export.
- **A/B testing (beta)** — a `track()` goal-event method across all four SDKs, a new events ingest
  endpoint, and an Experiments page computing conversion rate, uplift, and a two-proportion z-test
  with significance verdict.
- **Exposure dashboards** — per-flag variant distribution and a 14-day stacked timeline, derived
  from existing impression data.

## [0.1.19] - 2026-07-05

### Changed

- **Dashboard layout overhaul** — collapsible icon-only sidebar rail (persisted), and genuinely
  full-width list and table pages.
- Flag Editor and Settings restructured into two columns, separating behaviour from metadata.
- Setup and Login pages fill the window; `ProjectSettings` supports deep-linking via `?tab=`.

### Fixed

- **Setup/login redirect loop** — setup completion was inferred from an authenticated endpoint, so a
  fresh browser or expired session redirected permanently to `/setup`. Now uses a public endpoint.
- Dead "SDK Keys" link pointed at itself.
- React Native SDK shipped a stray local Gradle cache in the npm tarball.

## [0.1.18] - 2026-07-04

### Added

- **Prerequisite (dependent) flags** — recursive, with a depth guard that fails closed on cycles.
- **Weighted multivariate rollouts** — distribute traffic by weight, independent of the on/off gate.
- **`getValue`/`getVariant` in every SDK wrapper** — the bindings already supported multi-variant
  flags; the public wrappers only exposed `isEnabled`.
- **SDK impression reporting**, batched and off by default; attributes are never sent unless opted in.
- **Numeric targeting operators** alongside the existing string ones.
- **SDK resilience** — backoff with jitter on SSE reconnect, `onChange` listeners, offline
  persistence via a pluggable storage adapter, and an HTTP poll fallback.
- **Unified `connect()`/ready semantics** via a server-emitted `ready` event.
- **Flag lifecycle hygiene** — tags, owner email, and reversible archival, kept out of the
  evaluation core and wire format.
- **Cross-environment diff** with a one-click per-flag sync.
- **Scoped personal access tokens** — user-owned, revocable, optionally `read_only`, SHA-256 hashed.
- **Change requests** — a per-environment `require_approval` toggle routing flag edits through a
  different reviewer.

### Changed

- `evaluate`/`evaluate_variant` take a `&FlagStore` for recursive prerequisite lookups. Internal
  only; public SDK surfaces unaffected.

### Fixed

- Several timestamp fields serialized in a non-RFC-3339 format JavaScript's `Date` cannot parse,
  despite doc comments claiming ISO-8601.
- Node SDK omitted the Bearer token on SSE reconnect under `eventsource` v4.
- Dashboard called non-existent bare `/api/environments...` routes.
- **Node SDK packaging was broken since ~v0.1.17** — the hand-written wrapper and the generated
  multi-platform loader collided on one filename, so every install would have thrown
  `Cannot find module './index.node'`. Fixed, with a `require()` smoke test in the release workflow.
- Removed stale git-tracked copies of generated `rust-core/` build artifacts.

### Security

- Personal access tokens are hashed at rest and scoped to the owner's real role and project
  memberships; a `read_only` token cannot mint a `read_write` replacement.
- Change-request self-approval is rejected.

## [0.1.17] - 2026-06-14

- Audit logs, user segmentation, webhooks with HMAC signing, time-based scheduled flag changes, and
  live SSE connection monitoring (SDK Health).

## [0.1.0] - 0.1.16

See individual SDK changelogs (e.g. `sdks/flutter/dart/CHANGELOG.md`) and `docs/roadmap.md` for
earlier history: initial local-evaluation core, multi-variant flags, editor RBAC, projects/multi-
tenancy, impression tracking, and security hardening.
