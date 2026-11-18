# SDK compatibility: v0.1.31

Officially supported SDKs (as published): `@checkgate/node`, `@checkgate/web`
(also the engine of `@checkgate/edge` and `@checkgate/ssr`),
`@checkgate/react-native` and `checkgate_flutter`. All of them wrap the same
Rust core. No SDK was added. The SSE wire format and `/flags/snapshot` are
unchanged, so v0.1.30 SDKs work against a v0.1.31 server and v0.1.31 SDKs work
against a v0.1.30 server.

## Verification matrix (2026-11-18, local)

| Check | Node | Web / edge / ssr | React Native | Flutter |
|---|---|---|---|---|
| Unit tests | 21/21 (`node --test`) | 21/21 (vitest); edge and ssr unchanged | 21/21 (vitest, incl. 1 new) | 17/17 (`flutter test`, incl. 4 new) |
| Real engine (not mocked) | **6/6** `test/native-attributes.test.js` (new, in CI); differential vs v0.1.30 addon: 360 000 calls, 0 mismatches | **WASM smoke** `scripts/smoke-core.mjs` incl. new attribute case: pass | Rust FFI: `cargo test -p react-native` (6) + Miri in CI | Rust FFI: `cargo test -p flutter` (6) + Miri in CI |
| Initial connection + bootstrap against the **v0.1.31 server** | **pass** (`e2e/e2e.cjs`: SSE bootstrap, live UPSERT/PATCH, local evaluation) | — (same protocol; covered by the Node e2e and `tests/e2e_sse.rs`) | — | — |
| Update handling, ordering, staged reconnect | unit | unit | unit | unit + new chunk-boundary framing tests (R1) |
| Reconnection backoff (capped, jittered) | unit | unit | unit | unit |
| Poll fallback race (stale poll after reconnect) | unit | unit (`an outstanding poll cannot overwrite state after SSE takes over`) | unit | **new** unit (R3) |
| Missing flag → default | yes (`false` / `null` / given default) | yes | yes | yes |
| Invalid credentials (401) | retries forever (R6) | retries forever | retries forever | retries forever |
| Network failure | reconnect, then poll fallback | same | same; **polls if no `EventSource`** (R5) | same |
| Shutdown / cleanup | `disconnect()` flushes impressions | `disconnect()` | `disconnect()` | `close()` flushes impressions and events |

"—" means not run separately for that SDK: the same server endpoints and
protocol are exercised by the Node e2e and by the server's own SSE tests.

## Behavioural differences between SDKs (documented, not changed)

| Situation | Node | Web | React Native | Flutter |
|---|---|---|---|---|
| Evaluation before `connect()` resolves, no cached snapshot | `false` / `null` + warning | same | same | same |
| After `disconnect()` / `close()` | keeps serving the last flags | keeps serving | keeps serving | **returns `false`** (resets hydration) |
| Non-string attribute value, **referenced by a rule** | throws (napi conversion error) | skipped, treated as missing | skipped | skipped (Dart `Map<String, String>` normally prevents it) |
| Non-string attribute value, not referenced | ignored | skipped | skipped | skipped |
| Method to stop | `disconnect()` | `disconnect()` | `disconnect()` | `close()` |

In v0.1.30, web, RN and Flutter dropped **all** attributes when any one was
not a string (C3); this is fixed. Node still throws for a *referenced*
non-string attribute, as before; making the four SDKs identical here (throw
everywhere, or coerce numbers to strings everywhere) is a product decision
left for a future release.

## Changes in v0.1.31 that SDK users may notice

| SDK | Change |
|---|---|
| Node | `isEnabled` / `getValue` / `getVariant` are 1.9–8.8× faster. A stray non-string attribute that no rule reads no longer throws. Passing `null` for attributes no longer throws. |
| Web / edge / ssr | One non-string attribute no longer erases the others. |
| React Native | Same attribute fix. Without an `EventSource` polyfill, the SDK logs an error and polls (it used to hang `connect()`); **install `react-native-sse`** (README). |
| Flutter | Large bootstraps no longer lose flags or the `ready` event (`connect()` could hang). A slow poll can no longer roll back newer stream state, and polls time out after 30 s. Same attribute fix. |

## Not verified locally

- iOS and Android device builds of the React Native and Flutter SDKs (CI runs
  the Android native build; iOS needs a macOS runner with Xcode and is not in
  CI).
- A live end-to-end run of the web, RN and Flutter SDKs against a running
  server. Their transport code is covered by unit tests with fakes, and the
  protocol by the Node e2e and `server/tests/e2e_sse.rs`.
