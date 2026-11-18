# Developer experience: v0.1.31

The aim was to make existing functionality easier to adopt and to trust, not
to add features.

## Reviewed

| Workflow | Finding | Change |
|---|---|---|
| Reading the README / docs site | "Sub-microsecond evaluation" and "< 50 ms" appeared about 30 times: README, docs home page, guides, SEO metadata, SDK READMEs, JSDoc, dashboard setup page, overview diagram. Measured: Rust core 0.08–0.46 µs (true); Node SDK 7 µs in v0.1.30 and 0.8–3.9 µs now (false either way); web, RN and Flutter never measured; propagation p99 90–98 ms at 500–1 000 clients. | Each claim now matches its evidence: "well under a microsecond in the Rust core", "a few microseconds through the Node.js SDK", "typically within tens of milliseconds", with links to the benchmarks. The README's benchmark section (generated, validated in CI) is unchanged. |
| Comparison with other products (`docs/guide/what-is-checkgate.md`) | It gave competitors "~5–50 ms" with no source, and implied every competitor evaluates remotely. | Restated as Checkgate's measured numbers next to a factual description ("a network round trip per check when evaluated remotely"); no unsourced numbers about other products. |
| React Native install | React Native has no `EventSource`, the docs did not say to install one, and without it `connect()` never settled. | The install docs now require a polyfill (`react-native-sse`), with a code snippet; the SDK falls back to polling with an actionable error. |
| Evaluation semantics docs | The architecture page's evaluation diagram omitted prerequisites, which run before rules. | Diagram corrected, including the depth and budget bounds. |
| Configuration errors | New limits answer 422; operators need to know why. | Each rejection logs a `reason` (for example `segment "beta" is referenced more than once`), documented in OPERATIONAL-DIAGNOSTICS.md, plus a pre-upgrade SQL query in UPGRADE-AND-ROLLBACK.md. |
| Local testing on macOS | `scripts/docker-smoke.sh` aborted on macOS's stock bash 3.2 (`data[@]: unbound variable`). | Fixed; the full smoke suite runs locally on a Mac. |
| Before/after measurement | No tooling to compare two builds; the shared-target-dir trap could silently measure one build twice. | `bench/ab/` adds scripts for Node evaluation A/B, snapshot A/B, a Node addon differential check and upgrade/rollback, with a README that explains the trap. |
| SDK error handling | The Node SDK's JSDoc promised "<1 microsecond". | Corrected in `index.js` and `index.d.ts`. |

## Quick-start check (performed)

On a fresh database with the v0.1.31 release binary:
`GET /api/setup/key` with the setup token, `POST /api/setup/complete`, create
flags, connect `@checkgate/node`, evaluate. Performed both by
`bench/public/sdk/seed.mjs` and by the Node SDK e2e (`e2e/e2e.cjs`: pass). The
Docker quick start (`docker compose up`) is exercised by
`scripts/docker-smoke.sh` all-in-one (pass).

## Deliberately not changed

- **Making SDK behaviour uniform** (Flutter `close()` vs `disconnect()`,
  Node throwing on referenced non-string attributes): these are API changes,
  documented in SDK-COMPATIBILITY.md.
- **401/403 handling in SDKs** (R6): stopping retries and rejecting
  `connect()` would change public behaviour; recommended for a minor release
  with release notes.
- **New SDK languages**: none.
- **README restructuring**: the README already covers what it is, why, the
  capabilities, quick start, SDK usage, self-hosting, benchmarks and links.
  Only the inaccurate claims were edited.
