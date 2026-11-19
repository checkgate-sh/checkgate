---
title: "React Native SDK (JSI) — Feature Flags for iOS & Android"
description: "Integrate Checkgate's React Native SDK using JSI and C FFI for synchronous, in-process flag evaluation — no bridge, no request per decision. Android native build verified in CI; iOS not yet certified."
---

# React Native SDK (JSI)

The React Native SDK uses **C FFI** to call the Rust evaluation core directly from JavaScript via React Native's JavaScript Interface (JSI). This avoids the async bridge overhead and delivers synchronous, near-native evaluation performance.

## Installation

```bash
npm install @checkgate/react-native
# or
yarn add @checkgate/react-native
```

Requires `react-native` 0.71+ and React 18+; iOS 13.4+ and Android minSdk 23.

### Native setup (Rust toolchain)

The evaluation engine is Rust, shipped as source and compiled when you build the app, so the
build machine (including CI and EAS builders) needs [rustup](https://rustup.rs):

- **Android:** the Android targets and [cargo-ndk](https://github.com/bbqsrc/cargo-ndk). Gradle
  runs `cargo ndk` for the ABIs you build and links the result into `libcheckgate.so`.

  ```bash
  rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android
  cargo install cargo-ndk
  ```

- **iOS:** the iOS targets; a CocoaPods build phase compiles the engine for the SDK and
  architectures Xcode is building.

  ```bash
  rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
  ```

### iOS

```bash
cd ios && pod install
```

### Android

Gradle compiles the Rust engine with `cargo ndk` and links it via CMake; nothing to add to
`MainApplication`. Requires the toolchain above.

### Server-Sent Events polyfill

React Native has no built-in `EventSource`. Install one that accepts request
headers (the SDK authenticates the stream with an `Authorization` header) and
assign it to the global before calling `connect()`:

```bash
npm install react-native-sse
```

```typescript
import EventSource from 'react-native-sse';
(global as any).EventSource = EventSource;
```

Without it the SDK logs an error and keeps flags current by polling
`/flags/snapshot` every `pollIntervalMs` instead of streaming changes.

## Quick Start

```typescript
import { CheckgateNativeClient } from '@checkgate/react-native'

const client = new CheckgateNativeClient({
  serverUrl: 'https://flags.yourcompany.com',
  sdkKey: 'your-sdk-key',
})

// Connect on app start (typically in App.tsx)
useEffect(() => {
  client.connect()
  return () => client.disconnect()
}, [])

// Evaluate flags synchronously anywhere
const enabled = client.isEnabled('new-onboarding', userId, {
  plan: user.plan,
  country: user.country,
})
```

## API Reference

### `new CheckgateNativeClient(options)`

| Option | Type | Required | Description |
|--------|------|----------|-------------|
| `serverUrl` | `string` | Yes | Base URL of your Checkgate server |
| `sdkKey` | `string` | No | SDK key for authentication |
| `reconnectDelayMs` | `number` | No | SSE reconnect delay in ms (default: 5000) |
| `connectTimeoutMs` | `number` | No | Retry a stream attempt without `connected` after this long (default: 30000) |
| `streamIdleTimeoutMs` | `number` | No | Reconnect a stream silent this long (default: 3 × the announced heartbeat; `0` disables) |

### `client.connect(): Promise<void>`

Opens the SSE connection and resolves once the server has replayed the full flag set (the `ready`
event), like the Node and web SDKs. You may call it without awaiting: evaluations return `false`
for unknown flags until the bootstrap completes, and a rejection is never reported as unhandled.

### Connection health

`client.getStatus()` returns `{ state, lastSyncAt, error }`:

| `state` | Meaning |
|---|---|
| `'connecting'` | First connection attempt in progress |
| `'streaming'` | Live stream; flags are current |
| `'reconnecting'` | Stream lost; retrying with backoff (flags: last known) |
| `'polling'` | Stream keeps failing; polling `/flags/snapshot` |
| `'auth_failed'` | The server rejected the SDK key (401/403); not retrying |
| `'closed'` | After `disconnect()` |

`lastSyncAt` is when the client last confirmed its flags are current (a bootstrap, a poll, or any
event on a live stream, the server's heartbeats included), in ms since the epoch. Flags are always
served from the last known state; `Date.now() - lastSyncAt` is how old they may be. A stream that
receives nothing for 3 × the server's heartbeat interval (45 s by default) is dropped and
reconnected, so a silently dead connection (lost NAT mapping, half-open TCP) no longer looks healthy.

A rejected key is not retried: `connect()` rejects with a `CheckgateAuthError` (`err.status` is 401 or 403; the
key is never in the message), the last known flags keep being served, and calling `connect()` again
retries. `connect()` also rejects if `disconnect()` is called before it completes. While the server is
unreachable it keeps retrying; race it with a timeout if startup cannot wait.

### `client.isEnabled(flagKey, userKey, attributes): boolean`

Synchronous flag evaluation via JSI. Runs on the JS thread, calls into the Rust library without bridging overhead.

### `client.disconnect(): void`

Tears down the SSE connection. Call in your cleanup effect.

## Usage with React Navigation

```typescript
// App.tsx
import { NavigationContainer } from '@react-navigation/native'
import { CheckgateNativeClient } from '@checkgate/react-native'
import { createContext, useContext, useEffect, useState } from 'react'

const FlagContext = createContext<CheckgateNativeClient | null>(null)

export function useFlags() {
  return useContext(FlagContext)!
}

const flagClient = new CheckgateNativeClient({
  serverUrl: process.env.EXPO_PUBLIC_CHECKGATE_URL!,
  sdkKey: process.env.EXPO_PUBLIC_CHECKGATE_KEY,
})

export default function App() {
  useEffect(() => {
    flagClient.connect()
    return () => flagClient.disconnect()
  }, [])

  return (
    <FlagContext.Provider value={flagClient}>
      <NavigationContainer>
        {/* ... */}
      </NavigationContainer>
    </FlagContext.Provider>
  )
}
```

```typescript
// screens/HomeScreen.tsx
import { useFlags } from '../App'
import { useUser } from '../hooks/useUser'

export function HomeScreen() {
  const flags = useFlags()
  const user = useUser()

  const showNewUI = flags.isEnabled('new-home-ui', user.id, {
    plan: user.plan,
    beta: user.isBeta ? 'true' : 'false',
  })

  return showNewUI ? <NewHomeUI /> : <LegacyHomeUI />
}
```

## Expo Compatibility

The SDK is compatible with Expo managed workflow via the **Expo Dev Client**. It is not compatible with Expo Go (which doesn't support native modules).

```bash
npx expo install @checkgate/react-native
npx expo prebuild
```

`prebuild` and EAS builds compile the Rust engine too, so the build machine needs the toolchain
from [Native setup](#native-setup-rust-toolchain).

## Platform Support

| Platform | Architecture | Status in 0.1.31 |
|----------|-------------|--------|
| Android | arm64-v8a, armeabi-v7a, x86_64, x86 | Native build and link verified in CI; not tested on a device or emulator for this release |
| iOS | arm64 | **Not certified**: the iOS app build was not run for this release |
| iOS Simulator | x86_64 / arm64 | **Not certified** (as above) |

The JavaScript client (streaming, polling, reconnection) is covered by the shared SDK fault suite
and a live end-to-end run against a real server in CI.
