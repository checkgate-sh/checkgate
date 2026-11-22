<p align="center">
  <img src="https://raw.githubusercontent.com/checkgate-sh/checkgate/main/assets/logo.svg" width="64" alt="Checkgate">
</p>

# @checkgate/react-native

React Native SDK for [Checkgate](https://github.com/checkgate-sh/checkgate), the
self-hosted feature-flag platform. Flags are evaluated on the device by the
Checkgate Rust engine, called synchronously through JSI.

**Current release: 0.2.0.** No API changes since 0.1.31; the package is
versioned with the platform release. See the
[changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md).

## What it does

- A native `Checkgate` module installs a JSI host object backed by the Rust
  engine. `isEnabled()` and `getVariant()` call it synchronously on the JS
  thread: no bridge round trip and no network request per evaluation. User
  attributes stay on the device.
- Opens one Server-Sent Events (SSE) stream (through an `EventSource` polyfill
  you install). On every connect the server sends the complete current flag set
  of the SDK key's environment, then each change. A user who opens the app long
  after a flag was changed receives the flag as it is now.
- Keeps the flag set current: live updates, reconnection with backoff, and a
  polling fallback when the stream cannot be established.
- Optionally persists the last flag set through a storage adapter you pass (for
  example AsyncStorage), so a cold start can evaluate before the stream is up.
- Reports impressions and `track()` conversion events, batched and best-effort.

The engine ships as Rust source and is compiled when you build the app.

## Installation

```bash
npm install @checkgate/react-native react-native-sse
cd ios && pod install
```

Peer dependencies: `react` >= 18 and `react-native` >= 0.71. iOS deployment
target 13.4 or later; Android `minSdkVersion` 23 by default (read from your
project's `minSdkVersion`).

### Rust toolchain (build machine)

Every machine that builds the app, including CI and cloud builders, needs
[rustup](https://rustup.rs):

- **Android:** the Android targets and [cargo-ndk](https://github.com/bbqsrc/cargo-ndk).
  Gradle runs `cargo ndk` for the ABIs you build (`reactNativeArchitectures`) and
  links the result into `libcheckgate.so`.

  ```bash
  rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android
  cargo install cargo-ndk
  ```

- **iOS:** the iOS targets. A CocoaPods build phase compiles the engine for the
  SDK and architectures Xcode is building.

  ```bash
  rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
  ```

Autolinking registers the native module; there is nothing to add to
`MainApplication` or `AppDelegate`. The client installs its JSI bindings when it
is constructed.

### SSE polyfill (required for streaming)

React Native has no built-in `EventSource`. The SDK needs one that accepts
request headers, because it authenticates the stream with an `Authorization`
header. Assign it before calling `connect()`:

```js
import EventSource from 'react-native-sse'

global.EventSource = EventSource
```

Without a global `EventSource` the SDK logs an error and keeps flags current by
polling `GET /flags/snapshot` every `pollIntervalMs` instead of streaming.

## Quick start

```js
import EventSource from 'react-native-sse'
import { CheckgateNativeClient } from '@checkgate/react-native'

global.EventSource = EventSource

export const flags = new CheckgateNativeClient({
  serverUrl: 'https://flags.example.com', // no trailing slash
  sdkKey: 'sk_live_...',                  // the environment's SDK key (public, see Security)
})
export const flagsReady = flags.connect()

// Anywhere, synchronously:
const showNewOnboarding = flags.isEnabled('new-onboarding', user.id, { plan: user.plan })
```

### Re-rendering components

There are no React bindings in the package. `onChange` reports changes after
the initial load, so also re-render when `connect()` settles:

```js
import { useEffect, useReducer } from 'react'
import { flags, flagsReady } from './flags'

export function useFlag(flagKey, userKey, attributes) {
  const [, rerender] = useReducer((n) => n + 1, 0)
  useEffect(() => {
    let active = true
    const update = () => { if (active) rerender() }
    flagsReady.then(update, update)
    const off = flags.onChange(update)
    return () => { active = false; off() }
  }, [])
  return flags.isEnabled(flagKey, userKey, attributes)
}
```

## Configuration

`new CheckgateNativeClient(options)`:

| Option | Type | Default | Description |
|---|---|---|---|
| `serverUrl` | `string` | required | Base URL of the Checkgate server. |
| `sdkKey` | `string` | none | SDK key of the environment, sent as `Authorization: Bearer`. The server rejects requests without a valid key. |
| `reconnectDelayMs` | `number` | `5000` | Base reconnect delay; doubles per failed attempt. |
| `maxReconnectDelayMs` | `number` | `30000` | Upper bound of the reconnect delay (before jitter). |
| `pollFallbackThreshold` | `number` | `3` | Consecutive failed stream attempts before polling `GET /flags/snapshot`. `Infinity` disables polling. |
| `pollIntervalMs` | `number` | `30000` | Poll interval while polling. |
| `connectTimeoutMs` | `number` | `30000` | A stream attempt without the server's `connected` event within this time is abandoned and retried. |
| `streamIdleTimeoutMs` | `number` | 3 x server heartbeat | A connected stream with no event (heartbeats included) for this long is treated as dead and reconnected (45 s with the server's default 15 s heartbeat). `0` disables. No idle timeout against servers older than 0.1.31, which announce no heartbeat. |
| `reportImpressions` | `boolean` | `true` | Report each evaluation (flag key, user key, result) for analytics. |
| `impressionBatchSize` | `number` | `50` | Flush impressions (and events) once this many are buffered. |
| `impressionFlushIntervalMs` | `number` | `10000` | Periodic flush interval for impressions and events. |
| `sendEvaluationContext` | `boolean` | `false` | Include user attributes as `context` in impressions. Off, attributes never leave the device. |
| `storage` | `{ getItem, setItem }` or `null` | `null` | Adapter for persisting the last flag set (sync or async, for example `@react-native-async-storage/async-storage`). `null` persists nothing. |

The persisted entry is one JSON object of flag definitions keyed by flag key,
stored under `checkgate:flags:v2:<serverUrl>:<SHA-256 hex of the SDK key>`. It is
written after the initial flag set, each live change and each poll that changed
something, and read once per `connect()`; stored flags are evaluated until the
server's current set replaces them.

## API

| Member | Description |
|---|---|
| `connect(): Promise<void>` | Restores stored flags (if `storage` is set), opens the stream, resolves once the full flag set has been received (or a poll succeeded). Calling it again while pending returns the same promise. |
| `isEnabled(flagKey, userKey, attributes?): boolean` | Synchronous evaluation. `false` for unknown flags and before any flags are loaded. Attributes that are not strings are ignored. |
| `getVariant(flagKey, userKey, attributes?)` | `{ enabled, value }` for multi-variant flags, or `null` if the flag does not exist or nothing is loaded. |
| `getValue(flagKey, userKey, attributes?, defaultValue = null)` | The resolved value, or `defaultValue` when `getVariant()` would return `null`. |
| `onChange(cb): () => void` | `cb(flagKey)` for each flag created, updated or deleted after the initial load; after a reconnect, only for flags that changed meanwhile. Returns an unsubscribe function. |
| `track(eventKey, userKey, { value?, context? }?)` | Records a conversion event for experiments. Use the same `userKey` as for evaluation. |
| `getStatus()` | `{ state, lastSyncAt, error }`. `state`: `idle`, `connecting`, `streaming`, `reconnecting`, `polling`, `auth_failed` or `closed`. |
| `isReady()` / `isPolling()` | Whether the full flag set has been received / whether the client is polling. |
| `disconnect(): void` | Closes the stream, stops polling and timers, flushes pending impressions and events. |
| `CheckgateAuthError` | Error class (`name`, `status` 401 or 403) for a rejected SDK key. |

The package ships no TypeScript declarations.

## Reconnection

A stream error, connect timeout or idle stream closes the stream and the SDK
reconnects after `reconnectDelayMs x 2^attempt`, capped at
`maxReconnectDelayMs`, plus up to 30% jitter. After `pollFallbackThreshold`
consecutive failures it also polls `GET /flags/snapshot` while still retrying
the stream; a successful connection stops polling. Each reconnect receives the
full current flag set, staged and swapped in whole when complete. Change events
carry revisions; late or duplicate ones are ignored.

## Errors and lifecycle

- `connect()` rejects with `Native module not found` when the JSI bindings are
  not installed: the app was not rebuilt after installing the package (run
  `pod install` on iOS), or it runs in a host without the native module, such as
  Expo Go. Expo apps need a development build (`npx expo prebuild`), whose build
  machine needs the Rust toolchain above; Expo is not tested in CI.
- `connect()` rejects with `CheckgateAuthError` when the server answers 401 or
  403 (on the stream as reported by the polyfill, or on a poll). The client stops
  retrying, keeps serving the flags it has, and `getStatus()` reports
  `auth_failed`. Call `connect()` again to retry.
- `connect()` rejects with an `Error` if `disconnect()` is called first. It does
  **not** reject while the server is unreachable; it keeps retrying. Calling it
  without awaiting never produces an unhandled rejection.
- Once flags are loaded, `isEnabled()` throws if `flagKey` or `userKey` is not
  a string; `getVariant()` logs the error and returns `null`.
- Listener exceptions are caught and logged; storage errors are logged and
  ignored. Impressions and events are buffered (at most 10,000 each), sent in
  batches of up to 500 and not retried; `track()` before the stream has connected
  drops the event with a warning.
- Create one client per app. Call `disconnect()` when you no longer need it
  (for example in a root component's cleanup).

## Security considerations

- **The SDK key ships in your app bundle and is public.** Checkgate has one kind
  of SDK key, bound to one environment. It can only open the stream, read the
  flags snapshot and flag definitions of its environment, and submit impressions
  and events for it. It cannot create or change flags, read other environments,
  or call any other API (403). Use a separate key per app so you can revoke it
  alone; a revoked key gets 401.
- **Flag definitions reach the device.** Every flag of the environment,
  including targeting rule values and segment contents, is sent to the app, and
  stored as plain JSON by the `storage` adapter if you pass one. Do not put
  secrets or personal data you would not publish into flags of an environment
  whose key ships in an app.
- **Client-side flags are not access control.** Enforce authorization on your
  server.

## Platform support

| Platform | Status in 0.2.0 |
|---|---|
| Android (arm64-v8a, armeabi-v7a, x86, x86_64) | The packed npm package's Rust engine and JSI host are built and linked into `libcheckgate.so` in CI for every ABI. Not tested on a device or emulator. |
| iOS (device and simulator) | **Not certified.** No iOS build runs in CI. |

The JavaScript client (streaming, polling, reconnection, auth failures) is
covered in CI by unit tests, the shared SSE fault-injection suite, and a live
end-to-end run against a real server with `react-native-sse`, on Node.js with the
engine compiled to WebAssembly standing in for JSI.

## Documentation

- [React Native SDK guide](https://checkgate-sh.github.io/checkgate/sdks/react-native)
- [SDK keys and authentication](https://checkgate-sh.github.io/checkgate/api-reference#authentication)
- [SSE stream protocol](https://checkgate-sh.github.io/checkgate/api-reference#sse-stream)
- [Reliability and sync](https://checkgate-sh.github.io/checkgate/operations/reliability)
- [SDK diagnostics](https://checkgate-sh.github.io/checkgate/operations/sdk-diagnostics)
- [Changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md)

## License

Apache-2.0
