# SDK fault-injection suite

One suite, run by every SDK: randomized, reproducible SSE faults against the
real client code and its real transport, checking the same invariants.

| SDK | Transport under test | Engine | Runner |
|---|---|---|---|
| Node | `eventsource` (the SDK's dependency) | native addon if built, else stand-in | `sdks/nodejs/test/faults.test.mjs` |
| Web | `eventsource`, made to hide HTTP status like a browser | stand-in (WASM is stubbed in vitest) | `sdks/web/test/faults.test.js` |
| React Native | `react-native-sse` (the polyfill the README requires) over a streaming XHR like React Native's | stand-in (JSI is device-only) | `sdks/react-native/test/faults.test.js` |
| Flutter | the SDK's own `package:http` loop and SSE parser | real Rust engine with `CHECKGATE_FLUTTER_LIB` | `flutter.mjs` (drives `sdks/flutter/dart/tool/conformance_adapter.dart`) |
| Python | the SDK's own `http.client` loop and SSE parser | real Rust engine (PyO3 extension module) | `python.mjs` (drives `sdks/python/tool/conformance_adapter.py`) |

The stand-in engine (`fake-core.mjs`) is deterministic and evaluates enough
(enabled, rollout, one attribute rule) for the flapping check to bite. Engine
semantics themselves are tested in `core/` and by the live end-to-end suites.

None of this is a device test: React Native and Flutter run on the host.
The Flutter and Python clients run in a child process (one per scenario) that
speaks JSON lines with the runner.

## What is checked

`fault-server.mjs` speaks the server's SDK protocol (`/stream`, `/flags/snapshot`)
and can, between any two bytes: close every stream, stall them silently with the
socket open (half-open, lost heartbeats), cut an event or a bootstrap replay
mid-frame, duplicate an event, replay an older revision after a reconnect,
fragment writes to one byte each, delay them, refuse connections (restart), answer
503, delay snapshot responses, hold a new stream's bytes after reading its replay
(so polls overtake it: R20), and reject the key (401). `/flags/snapshot?v=2`
answers with revisions and tombstones as a 0.1.31 server does; with
`legacySnapshot` it answers the bare array of older servers.

`scenario.mjs` samples the client every 25 ms and requires:

| | Invariant |
|---|---|
| I1 | every flag version served existed on the server (no partial or corrupt state) |
| I2 | per flag, the revision never goes back; a deleted flag only returns at a newer revision |
| I3 | a flag disappears only after the server deleted it |
| I4 | at an unchanged flag version, evaluation is unchanged for the same user (no flapping from reconnects) |
| I5 | at most 2 streams at once (one may still be closing), exactly 1 after recovery, 0 after close |
| I6 | `connect()` settles: resolves, or rejects on a rejected key |
| I7 | after faults heal, the client serves exactly the server's flags within `CONVERGE_MS` |

plus, in named scenarios: a delayed replay never rolls back or resurrects what newer
polls delivered (R20, the deterministic forms of seeds 173 and 184); a silent stream is replaced within 3 heartbeats + 400 ms;
a quiet healthy stream (new or pre-0.1.31 server) is never replaced; a revoked key
reaches `auth_failed`, makes no further requests, and recovers on `connect()`.

## Running

```bash
# CI tier: the named scenarios + ci-seeds.json (about 1 minute per SDK)
(cd sdks/nodejs && node --test test/faults.test.mjs)
(cd sdks/web && npx vitest run test/faults.test.js)
(cd sdks/react-native && npx vitest run test/faults.test.js)
(cd sdks/flutter/dart && flutter pub get) && cargo build -p flutter && \
  CHECKGATE_FLUTTER_LIB=$PWD/target/debug/libflutter.dylib node --test sdks/conformance/flutter.mjs
(cd sdks/python && maturin develop --release) && \
  node --test --test-concurrency=1 sdks/conformance/python.mjs   # PYTHON=… to pick the interpreter

# Extended tier: any seed range (nightly in .github/workflows/reliability.yml)
CONFORMANCE_SEEDS=1-500 CONFORMANCE_TRACE_DIR=/tmp/traces CONFORMANCE_REPORT=/tmp/results.jsonl \
  node --test sdks/nodejs/test/faults.test.mjs

# Reproduce one failure
CONFORMANCE_SEEDS=11 node --test sdks/conformance/flutter.mjs

# Timeline of one seed or named scenario against the Node SDK (or an older build)
node sdks/conformance/trace-r20.mjs 173 [path/to/sdks/nodejs/index.js]
```

A failure prints the seed, the violations and the step trace, and writes them to
`$CONFORMANCE_TRACE_DIR/<label>.json`. `CONFORMANCE_REPORT` appends one line per
passing scenario with its recovery time and server counters.

When the extended tier finds a failing seed, fix the cause and add the seed to
`ci-seeds.json` (`seeds` and a `regressions` note), so every pull request runs it.

## Timing

Production timing scaled down (`TIMING` in `scenario.mjs`): heartbeat 100 ms
(idle deadline 300 ms), reconnect backoff 20–200 ms, connect deadline 500 ms,
poll every 100 ms after 3 failures. The rules are the production rules; only
the constants differ.
