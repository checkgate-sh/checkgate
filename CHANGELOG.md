# Changelog

All notable changes to Checkgate are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

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
