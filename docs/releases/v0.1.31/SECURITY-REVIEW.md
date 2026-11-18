# Security review: v0.1.31

Scope: a threat-model review of the server (`server/`), the evaluation core
(`core/`) and the SDK bindings, starting from v0.1.30 (`8ef6452`, tree-identical
to `origin/dev` at `f38f02e`). It focuses on weaknesses that can be shown in
code, not on theoretical ones. Each finding was traced to the source; the
fixed ones have regression tests.

Earlier passes (v0.1.23 to v0.1.30) already covered these, and they were not
redone: pagination clamping on the flag list, the CSRF header, the SSE
connection cap and bootstrap semaphore, atomic first-run setup with a one-time
token, client-IP resolution through trusted proxies, per-class rate limits,
SDK-key scoping for ingest and health, connection-string redaction, the
non-root container, and FFI `// SAFETY:` comments with Miri.

## Threat model

| Actor | Can do legitimately | Must not be able to |
|---|---|---|
| Anonymous network client | Log in, complete setup with the setup token, call `/health*` | Read or change any flag, learn which accounts exist, exhaust the server |
| SDK key holder (keys ship in browser and mobile bundles, so treat them as public) | Stream, snapshot and read flags of **one** environment, ingest impressions and events | Reach another environment, any admin route, or personal data |
| Viewer | Read the projects they belong to | Write anything, read secrets |
| Editor | Change flags, segments, experiments and schedules in their projects | Affect other projects, exhaust shared server memory, or freeze every SDK |
| Admin | Manage environments, keys, users, webhooks and integrations | Reach infrastructure credentials through the server |
| Personal access token | Act as its user, capped by its scope | Exceed its scope or outlive its expiry |

## Findings and disposition

Severity reflects impact on a self-hosted deployment where every project shares
one server.

| ID | Finding | Severity | Status | Fix / evidence |
|---|---|---|---|---|
| S1 | **Segment-expansion memory amplification.** A flag could reference one segment thousands of times. The server expands every reference (`segments::expand_flag_with_segments`) for each publish, SSE bootstrap and snapshot: about 3 000 references to a 60 KB segment make a flag of roughly 180 MB, from two requests under the 64 KiB body cap. | **High** (any editor can exhaust memory for all projects) | **Fixed** | `server/src/limits.rs`: duplicate segment references are rejected; at most 50 distinct segments, 500 rules per flag and 500 per segment. Applied on create, on every patch path (direct, scheduled, change-request approval) and on segment writes. Tests: `limits::tests::*`, `tests/hardening.rs::configuration_limits_reject_amplifying_writes`. |
| S2 | **Prerequisite fan-out makes SDK evaluation exponential.** Prerequisites that are all satisfied never short-circuit, so 10 flags that each list the next one 8 times cost about 8⁹ ≈ 134 M evaluations per `isEnabled` in every SDK. | Medium (an editor can freeze every client application) | **Fixed** | Core: `MAX_PREREQUISITE_EVALUATIONS = 1000` per evaluation, failing closed like the depth guard. Server: self-references and duplicates are rejected, at most 50 per flag. The regression test took more than 120 s before the fix and now finishes in milliseconds: `evaluator::tests::test_prerequisite_fan_out_is_bounded_and_fails_closed`. |
| S3 | **Webhook SSRF that returns the full response, via a redirect.** The delivery client followed up to 10 redirects to any host. A 303 to a cloud-metadata address became a GET, and the uncapped response body was stored where project viewers can read it. | Medium (needs an admin, but escalates to infrastructure credentials) | **Fixed** | `webhook_fire::delivery_client()` follows no redirects (a 3xx is logged as a 3xx). Response bodies are read through a 1 KiB cap (`read_capped_body`). Tests: `webhook_fire::tests::{redirects_are_not_followed, stored_response_bodies_are_capped}`. Still open: an admin can point a webhook **directly** at a private address; see "Not fixed". |
| S4 | **Integration URL (a posting credential) leaked through delivery errors.** reqwest's error text ends with the full URL. It was stored in `error` (readable by viewers and read-only PATs) and logged, although the API otherwise shows only a preview of the URL. | Low–Medium | **Fixed** | `e.without_url()` before the error is stored or logged, for integrations and webhooks. Webhook URLs are no longer logged. |
| S5 | **A read-only PAT could revoke the user's other tokens and mint a token that never expires.** | Low | **Fixed** | `DELETE /api/tokens/{id}` now requires write scope. A PAT cannot mint a token that outlives itself. Test: `tests/hardening.rs::tokens_cannot_outlive_or_revoke_beyond_their_scope`. |
| S6 | **Audit-log paging used raw `limit`/`offset`.** A huge `limit` returned the whole history with before/after JSON; a negative value caused a 500. The impressions offset was uncapped. | Low | **Fixed** | Clamped like the flag list (`MAX_PAGE_LIMIT` 500, `MAX_PAGE_OFFSET` 1 000 000). Test: `tests/hardening.rs::audit_paging_is_clamped`. |
| S7 | **`?sdk_key=` written to the server's own logs.** The default trace span records the full URI, and the JSON log layer repeats the span on every event. | Low | **Fixed** | The request span records the path only (`main.rs`, `make_span_with`). |
| S8 | **One non-string attribute erased all attributes in the web, React Native and Flutter cores** (`from_value(..).unwrap_or_default()`). Because a missing attribute satisfies `not_equals`, a user excluded by `plan not_equals free` was let through. | Medium (a targeting bypass that fails open) | **Fixed** | `checkgate_core::attributes`: string values are kept and other values are skipped one by one. Tests: `attributes::tests::*` (including the excluded-user case) and the WASM smoke check in `sdks/web/scripts/smoke-core.mjs`. |
| S10 | **Unauthenticated login requests could stall the instance.** Argon2 verification ran on async worker threads, so concurrent logins from several IPs (each within the per-IP auth rate limit) could occupy every worker. | Medium (availability) | **Fixed** | See RELIABILITY-REVIEW.md R18: blocking pool, at most one hash per CPU. |
| S9 | `cargo audit`: **RUSTSEC-2026-0538** (`quinn-udp` 0.5.14, receive loop spin), published after v0.1.30. | Info (only in the lockfile: reqwest's HTTP/3 path is not compiled) | **Fixed** | Lockfile bumped to 0.5.16. `cargo audit` is clean; the two existing documented ignores (`rsa`, `libsqlite3-sys`) are still lockfile-only. CI's `supply-chain` job would have failed on its next run. |

### Not fixed in v0.1.31

These are documented here, none is critical or high, and the release does not
depend on them.

| ID | Finding | Severity | Why deferred / mitigation |
|---|---|---|---|
| N1 | Webhook and integration URLs may point **directly** at loopback, link-local or RFC 1918 addresses (integrations allow `http://localhost` on purpose). | Low (admin only; redirects and full bodies are closed by S3) | Blocking these needs DNS-pinned address filtering and an operator opt-in for internal receivers, which is a feature. Mitigation: restrict egress at the network layer. |
| N2 | REST flag reads with an SDK key return `owner_email` and tags. | Low | SDK keys are public. Removing these fields from SDK-key responses could break consumers that read tags with an SDK key; this needs a deprecation note first. Recommended for v0.1.32. |
| N3 | Revoking an SDK key, or removing a project member, does not close streams already open with it. They keep receiving updates until they reconnect. | Low | Needs connection tracking by key or user id. Mitigation: restart instances after an urgent key revocation. |
| N4 | Login lockout is keyed only on email: 5 failed attempts every 10 minutes from anyone locks the account out. The check and the increment are separate statements. | Low | Changing the lockout policy needs a product decision (per IP plus per email, or backoff). |
| N5 | SDK keys are stored in plaintext (PATs are SHA-256 hashed). | Info | This is a design note, not a vulnerability; the keys are public by design. |
| N6 | Impressions and events have no retention or prune job, so a public SDK key can grow these tables within the ingest rate limit. | Low | Needs a retention setting. Mitigation: prune with SQL, or the ingest rate limits in `docs/operations`. |

## Checked and found correct

- **Router groups match their role layers** (`main.rs`). Every editor route
  carries `{env_id}`, and `require_editor` enforces a project write role on it.
- **Every sub-resource query filters by `environment_id`**: change requests,
  scheduled changes, segments, experiments, webhooks, integrations and their
  deliveries.
- **SDK keys** are confined to stream, snapshot, flag reads and ingest of their
  own environment, both in the route allowlist and in each handler. Admin
  routes reject them. Covered by `tests/environment_isolation.rs`, which runs
  in CI.
- **PATs** are accepted only in headers. They take the user's current role on
  each request, and a read-only PAT cannot mint a read-write one.
- **Change requests block self-approval.** With `require_approval`, every
  write path (patch, delete, overwrite-create, promote, scheduled changes,
  segments) returns 409 or is queued.
- **Cookies** are `HttpOnly`, `SameSite=Strict` and `Secure` by default.
  `/metrics` is off by default and served on its own listener. A release build
  refuses to start without `SESSION_SECRET`.
- **Login** verifies against a dummy hash for unknown accounts, so response
  timing does not reveal which emails exist.

## Tools

- `cargo audit`: clean after S9. Advisory DB of 2026-11-18, 1 533 advisories.
- `cargo deny check`: advisories, bans, licenses and sources all pass.
- GitHub Dependabot shows 3 open alerts on `google.golang.org/grpc` in
  `integrations/terraform-provider-checkgate` (#51, #54, #55; two rated high).
  **They are stale.** `go.mod` and `go.sum` pin v1.85.0, and `go list -m`
  resolves v1.85.0, which is at or above every patched version (1.82.2,
  1.83.1). The alerts are raised against `main`'s dependency graph, which
  lags; dismiss them as "fix already applied" after confirming on `main`.
  **RC2: root cause found.** GitHub's dependency graph (SBOM regenerated
  2026-11-18) still lists grpc v1.82.1 for that manifest: its static parser
  does not resolve these Go modules, which reach the shared client through a
  `replace` to a local path, so it never picked up the 2026-11-11 bump. The new
  `dependency-submission.yml` workflow submits the graph `go` resolves, on
  pushes to `main`; the alerts close once it has run there.

## RC2 changes (reviewed)

- **SSE lifecycle and auth handling** (SDK-CONNECTION-RELIABILITY.md): no new
  retry amplification (deadlines reuse the jittered backoff; the browser
  credential check is at most one request per failed attempt); one watchdog
  per client, cleared on close; auth errors carry the status, never the key
  (asserted in tests); `heartbeat_ms` is server-controlled (1–300 s).
- **Heartbeat**: 27 bytes per quiet interval per stream; no new endpoint, no
  new input parsed by the server.
- **Fault-test infrastructure**: test-only (`sdks/conformance`, test files,
  `bench/`); nothing ships in a package (`files` lists unchanged). New dev
  dependencies: `eventsource` (web), `react-native-sse` (RN). `npm audit`
  reports 4–5 high advisories in the web and RN dev trees, all one ReDoS in
  `picomatch` reached through `vitest`, present before RC2, dev-only, with no
  fixed release yet (affected range `*`).
- **All-in-one supervisor**: capabilities reduced to KILL, SETUID, SETGID and
  `no_new_privs` (DEPLOYMENT-READINESS.md).
- **Cross-tenant exposure and rollout determinism**: untouched (no server
  query, key handling or core change in RC2; the 22 fault scenarios check
  evaluation stability at every unchanged revision).
- `cargo clippy --workspace --all-targets --all-features -D warnings`: clean,
  under the workspace's deny-level safety lints.
- Miri runs on the FFI crates in CI (job `miri`). The FFI changes in this
  release only swap the attribute decoder, inside existing safe code.
