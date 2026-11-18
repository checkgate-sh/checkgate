<p align="center">
  <img src="../../assets/logo.svg" width="64" alt="Checkgate">
</p>

# @checkgate/react-native

**Checkgate React Native SDK (JSI)** — The official high-performance mobile client for Checkgate.
Powered by a direct JSI (JavaScript Interface) natively bridging to Rust, this SDK completely avoids strictly serialized asynchronous React Native bridge delays, allowing synchronous, in-process feature toggle evaluations on device.

## Installation

```bash
npm install @checkgate/react-native
# iOS requires pod installation
cd ios && pod install
```

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

### Native setup

The evaluation engine is Rust, shipped as source in the package and compiled
when you build the app, so the build machine needs a Rust toolchain
([rustup](https://rustup.rs)):

- **Android:** the Android targets and [cargo-ndk](https://github.com/bbqsrc/cargo-ndk).
  Gradle runs `cargo ndk` for the ABIs you build (`reactNativeArchitectures`)
  and links the result into `libcheckgate.so`.

  ```bash
  rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android
  cargo install cargo-ndk
  ```

- **iOS:** the iOS targets. A CocoaPods build phase compiles the engine for the
  SDK and architectures Xcode is building.

  ```bash
  rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
  ```

Autolinking registers the native `Checkgate` module; there is nothing to add to
`MainApplication` or `AppDelegate`. The client installs its JSI bindings the
first time it is constructed. If `connect()` rejects with *Native module not
found*, rebuild the app (and re-run `pod install` on iOS).

## Quick Start

Plug the incredibly fast evaluating `CheckgateProvider` context or native clients directly into your React Native app.

```javascript
import { CheckgateNativeClient } from '@checkgate/react-native'

const client = new CheckgateNativeClient({
  serverUrl: 'https://checkgate.your-company.com',
  sdkKey: 'sk_live_xxxxxx'
})

// Run this during app initialization
client.connect()

// Later natively inside your RN component lifecycle
function CheckoutScreen({ user }) {
  const showApplePay = client.isEnabled('apple-pay-checkout', user.id, { plan: user.plan })
  
  return showApplePay ? <ApplePayButton /> : <StandardCheckout />
}
```

## Connection health

A stream that goes silent for 3 server heartbeats (45 s by default) is dropped
and reconnected. A rejected SDK key (HTTP 401/403) is not retried: `connect()`
rejects with `CheckgateAuthError` and the last known flags keep being served.
`client.getStatus()` returns `{ state, lastSyncAt, error }` so you can tell how
fresh the flags are. See the [SDK docs](https://checkgate-sh.github.io/checkgate/sdks/).

## A/B testing: track conversions

Record goal events with `track()` to measure how each flag variant converts. Pair
a flag with a goal event in an **Experiment** in the dashboard to see per-variant
conversion rates and statistical significance.

```javascript
const variant = client.getVariant('checkout-button-color', user.id)

// …later, when the user converts:
client.track('checkout_complete', user.id, { value: 49.99 })
```

Events are buffered and reported asynchronously (best-effort), just like impressions.
Use the **same `userKey`** you pass to `getVariant()`/`isEnabled()` so conversions
attribute to the right variant.

## Why Checkgate JSI?
* **Synchronous Native Bridge:** Unlike standard HTTP wrappers, Checkgate's JSI evaluates flag arrays in memory avoiding async JS bridge loading.
* **Bandwidth Conscious:** Streamlined SSE prevents apps from polling the network heavily on metered mobile plans.

Browse the [official Checkgate documentation](https://checkgate-sh.github.io/checkgate) for architecture schemas and more.
