# checkgate_flutter

Flutter SDK for [Checkgate](https://github.com/checkgate-sh/checkgate) — local feature-flag evaluation via Rust FFI.

Flags are evaluated **in-process** (sub-microsecond, no network round-trip). The SDK opens a persistent SSE stream to your Checkgate server and keeps the local flag store up to date in real time.

## Installation

```yaml
dependencies:
  checkgate_flutter: ^0.1.0
```

## Usage

```dart
import 'package:checkgate_flutter/checkgate_flutter.dart';

final client = CheckgateClient(
  serverUrl: 'https://flags.example.com',
  sdkKey: 'your-sdk-key',
);

await client.connect();

final enabled = client.isEnabled('dark_mode', userId, {'country': 'US'});
```

### API

| Method | Description |
|--------|-------------|
| `connect()` | Opens the SSE stream and starts receiving flag updates. |
| `isEnabled(flagKey, userKey, [attributes])` | Evaluates a flag locally. Returns `false` if not initialised. |
| `close()` | Cancels the SSE subscription and frees resources. |

## How it works

1. On `connect()`, the SDK connects to `<serverUrl>/stream` via SSE.
2. The server sends a full flag snapshot on connect, then incremental `update` events.
3. Each update is applied to the Rust-backed in-memory store via FFI.
4. `isEnabled` evaluates the flag entirely in Rust — no async, no network.

## License

Apache-2.0
