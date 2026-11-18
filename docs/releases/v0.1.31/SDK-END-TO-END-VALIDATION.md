# SDK end-to-end validation: v0.1.31 RC2

What ran against a real server, on which runtime, and what did not. "Live"
means the SDK's real client code, its real transport and a real Rust engine,
against a real Checkgate server, PostgreSQL 16 and Redis 7.

## Live suite (`sdks/conformance/live.mjs`, `scripts/sdk-live-e2e.sh`)

Server: debug build of the candidate, fresh database, `SSE_HEARTBEAT_INTERVAL_SECS=1`
(idle deadline 3 s). Each SDK connects through an HTTP proxy that can sever
the link, stall open streams (sockets left open, no bytes: a half-open
connection) and refuse only `/stream`. Client timing: reconnect 100 ms → 1 s,
poll fallback after 2 failures, every 500 ms; connect deadline 3 s.

| Check | Node | Web | React Native | Flutter |
|---|---|---|---|---|
| Runtime / transport | Node 22, `eventsource` | Node 22, `eventsource` made to hide HTTP status like a browser | Node 22, **real `react-native-sse` 1.2.1** over a streaming XHR like React Native's | Dart VM, the SDK's `package:http` loop |
| Engine | native addon (napi) | WASM (`wasm-pack` build) | WASM behind the JSI bridge interface | Rust over FFI (host `cdylib`) |
| Bootstrap: `connect()` resolves, flag evaluates, state `streaming` | pass | pass | pass | pass |
| Create → targeting (`plan=pro` matches, rollout 0 excludes) → patch → delete delivered | pass | pass | pass | pass |
| Reconnect after the link drops; change made meanwhile arrives | pass | pass | pass | pass |
| Half-open stream detected and replaced; change arrives | pass (3.12 s) | pass (3.09 s) | pass (3.09 s) | pass (3.13 s) |
| Poll fallback with only `/stream` blocked, then back to streaming | pass | pass | pass | pass |
| Invalid key: `connect()` rejects with the auth error, `auth_failed`, no retries in 2 s | pass (1 request) | pass (2: stream + credential check) | pass (1) | pass (1) |
| Revoked key (real `DELETE /api/projects/{id}/keys/{id}`): terminal after the next attempt, last flags kept, no retries | pass | pass | pass | pass |
| Shutdown releases every stream | pass | pass | pass | pass |

Times in the table are from the first run of the suite.

**Final run on the release candidate** (`3c32cbe`, fresh database, 2026-11-19
19:19–19:20 UTC): **32 of 32 checks passed**, 8 per SDK. Half-open stream
replaced after 3.12 s (Node), 3.07 s (web), 3.15 s (React Native), 3.13 s
(Flutter); reconnect after a dropped link 102–158 ms; revoked key detected on
the next attempt 150–173 ms after the link was cut; invalid key: 1 request
(web: 2). Flutter's 2.6 s bootstrap includes starting the Dart VM adapter. CI runs the same suite for all four
SDKs on Linux (`SDK e2e (live SSE)` job). The existing native Node e2e
(`sdks/nodejs/e2e/e2e.cjs`) still runs before it.

## Fault suite (`sdks/conformance`, every SDK)

Randomized and named fault scenarios against each SDK's real client and
transport. See SSE-FAULT-INJECTION.md.

## iOS

| | Flutter | React Native |
|---|---|---|
| Rust engine cross-compiles: `aarch64-apple-ios`, `aarch64-apple-ios-sim`, `x86_64-apple-ios` | pass | pass |
| Every symbol the bindings/header declare is exported (7 Dart bindings / 8 in `checkgate_core.h`) on all three | pass | pass |
| Native glue compiles for iOS | n/a (pure Dart) | JSI host (`cpp/checkgate_jsi.cpp` + RN 0.76.9 `jsi.cpp`) compiles and links against the Rust library with no undefined symbol, for all three targets. **`ios/Checkgate.mm` not compiled** (needs React's headers through CocoaPods) |
| App build | `flutter build ios --simulator --debug`: pass | **not run** |
| Runs on iOS | **pass**, iOS 17.0.1 simulator (x86_64): connect 97 ms, state `streaming`, targeting evaluated by the statically linked engine, live update 118 ms, invalid key → `CheckgateAuthException` / `authFailed`, `close()` | **not run** |

Environment: macOS 15 (Intel), Xcode 16.2, Flutter 3.38.7, iOS SDK 18.2
simulator runtimes. **CocoaPods on this machine is broken** (installed for an
older Ruby; the gem is missing under Ruby 4.0), which ruled out
`integration_test` (a plugin) for Flutter and any React Native app build
(`pod install`). The Flutter check therefore ran as a plain app that prints
its results, installed with `simctl` and read from its console.

Not verified: a physical device, a signed device build, an arm64 simulator
(Intel host), and anything React Native on iOS beyond compiling and linking
the JSI host. iOS is **not certified** for v0.1.31 on this evidence; the
Flutter result shows the SDK works on iOS when the engine is linked as
documented.

## Android

CI's `SDK native (react-native, Android)` job packs the React Native package,
builds its Rust engine for four ABIs and links `libcheckgate.so` with no
undefined symbol (unchanged in RC2). No Android emulator or device run, and no
Flutter Android build, in this RC.

## F1: the Flutter package could not be used as published (high, fixed in part)

Found while preparing the iOS check:

1. **Not importable.** Every published `checkgate_flutter` (0.1.21 to 0.1.30 on
   pub.dev; verified by listing the 0.1.30 archive) kept `checkgate_flutter.dart`
   at the package root. Dart only exposes `lib/`, so the documented
   `import 'package:checkgate_flutter/checkgate_flutter.dart'` fails to resolve
   in any app (verified with a fresh `flutter create` app). **Fixed:** the
   library is in `lib/`; the same import now resolves.
2. **No engine.** The package bundles no compiled library and no plugin that
   builds one (unlike the React Native podspec), while the docs listed every
   platform as supported. **Documented, not fixed:** `docs/sdks/flutter.md` now
   gives the verified iOS steps (static library + `-force_load`), the correct
   Android library name (`libcheckgate_flutter.so`; cargo builds `libflutter.so`),
   and which platforms were verified. A Flutter plugin that builds and bundles
   the engine is the proper fix and is a feature, not an RC change.

Severity: high for anyone adopting the Flutter SDK from pub.dev; no impact on
the server or the other SDKs. See RC2-RELEASE-READINESS.md for the release
decision.
