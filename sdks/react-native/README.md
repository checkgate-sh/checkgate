<p align="center">
  <img src="../../assets/logo.svg" width="64" alt="Checkgate">
</p>

# @checkgate/react-native

**Checkgate React Native SDK (JSI)** — The official high-performance mobile client for Checkgate.
Powered by a direct JSI (JavaScript Interface) natively bridging to Rust, this SDK completely avoids strictly serialized asynchronous React Native bridge delays, allowing sub-microsecond synchronous feature toggle evaluations natively on device.

## Installation

```bash
npm install @checkgate/react-native
# iOS requires pod installation
cd ios && pod install
```

## Quick Start

Plug the incredibly fast evaluating `CheckgateProvider` context or native clients directly into your React Native app.

```javascript
import { CheckgateNativeClient } from '@checkgate/react-native'

const client = new CheckgateNativeClient({
  serverUrl: 'https://checkgate.your-company.com',
  sdkKey: 'pk_mobile_xxxxxx'
})

// Run this during app initialization
client.connect()

// Later natively inside your RN component lifecycle
function CheckoutScreen({ user }) {
  const showApplePay = client.isEnabled('apple-pay-checkout', user.id, { plan: user.plan })
  
  return showApplePay ? <ApplePayButton /> : <StandardCheckout />
}
```

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
