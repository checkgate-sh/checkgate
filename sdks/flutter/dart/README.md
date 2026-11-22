# checkgate_flutter

Flutter and Dart client for [Checkgate](https://github.com/checkgate-sh/checkgate)
feature flags. Flags are evaluated in-process by Checkgate's Rust engine,
called through Dart FFI; the client keeps them current over Server-Sent Events.

Current version: **0.2.0**
([changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md)).
The Dart API is the same as in 0.1.31.

> **The package does not bundle a compiled engine.** It contains the Dart
> client and the engine's Rust source, but no prebuilt library and no Flutter
> plugin that builds one. You build the Rust engine and link it into your app
> yourself; see "Linking the native engine" below.

## What it does

- Evaluates flags synchronously in the Rust engine: no `await` and no network
  request per evaluation.
- Opens an SSE stream (`GET /stream`) in Dart, applies each full flag set as a
  whole on the server's `ready` event, and ignores late, duplicated or
  reordered changes by revision.
- Reconnects with exponential backoff and jitter, falls back to polling
  `GET /flags/snapshot` while the stream keeps failing, and replaces a stream
  that goes silent.
- Optionally persists the last flag set through a storage adapter you provide.
- Reports evaluation impressions (on by default) and goal events from
  `track()` for experiments.

## Installation

```yaml
dependencies:
  checkgate_flutter: ^0.2.0
```

```bash
flutter pub get
```

Then build and link the engine as described below: without it, `connect()` fails because the engine library cannot be
loaded.

## Minimal example

```dart
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:checkgate_flutter/checkgate_flutter.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();

  final flags = CheckgateClient(
    serverUrl: 'https://flags.example.com',
    sdkKey: 'sk_live_your_key',
  );
  // Completes once the server has sent the full flag set. It keeps retrying
  // while the server is unreachable, so bound the wait.
  try {
    await flags.connect().timeout(const Duration(seconds: 10));
  } on TimeoutException {
    // Evaluations return false / null until the flags arrive.
  }

  runApp(MyApp(flags: flags));
}

class MyApp extends StatelessWidget {
  const MyApp({super.key, required this.flags});

  final CheckgateClient flags;

  @override
  Widget build(BuildContext context) {
    final newCheckout = flags.isEnabled('new-checkout', 'user-123', {'plan': 'pro'});
    return MaterialApp(
      home: Text(newCheckout ? 'New checkout' : 'Classic checkout'),
    );
  }
}
```

## Configuration

`CheckgateClient({...})` takes named parameters:

| Parameter | Default | Description |
|---|---|---|
| `serverUrl` | required | Base URL of the Checkgate server |
| `sdkKey` | required | SDK key of the environment |
| `reconnectBaseDelay` | `Duration(seconds: 2)` | First reconnect delay; doubles per failed attempt, plus up to 30 % jitter |
| `maxReconnectDelay` | `Duration(seconds: 30)` | Cap of the reconnect delay |
| `pollFallbackThreshold` | `3` | Failed stream attempts before the client also polls `GET /flags/snapshot` |
| `pollInterval` | `Duration(seconds: 30)` | Poll interval while polling |
| `connectTimeout` | `Duration(seconds: 30)` | A stream attempt without the server's `connected` event in this time is abandoned and retried |
| `streamIdleTimeout` | `null` | A connected stream that receives no complete frame for this long is reconnected. `null`: 3 × the heartbeat the server announces; servers before 0.1.31 announce none and send a keep-alive comment every 15 s, so 45 s. `Duration.zero` disables |
| `storage` | `null` | A `CheckgateStorage` (`read(key)`, `write(key, value)`) that persists the last flag set |
| `reportImpressions` | `true` | Report evaluations (flag key, user key, value, time) to the server |
| `impressionBatchSize` | `50` | Flush buffered impressions and events once this many are queued |
| `impressionFlushInterval` | `Duration(seconds: 10)` | Periodic flush interval |
| `sendEvaluationContext` | `false` | Include user attributes in reported impressions |

## API overview

| Member | Description |
|---|---|
| `Future<void> connect()` | Loads the engine, hydrates from `storage`, opens the stream; completes after the full flag set. Returns at once if already connected |
| `bool isEnabled(flagKey, userKey, [attributes])` | Whether the flag is on; `false` for an unknown flag or before flags are loaded |
| `Map<String, dynamic>? getVariant(flagKey, userKey, [attributes])` | `{'enabled': bool, 'value': ...}`, or `null` for an unknown flag or before flags are loaded |
| `dynamic getValue(flagKey, userKey, [attributes, defaultValue])` | The resolved value, or `defaultValue` |
| `void track(eventKey, userKey, {num? value, Map<String, dynamic>? context})` | Queues a goal event for experiments |
| `void Function() onChange(callback)` | Calls `callback(flagKey)` after a flag is created, updated or deleted (not for the initial load); returns an unsubscribe function |
| `bool isReady` | Whether a full flag set from the server has been applied |
| `bool isPolling` | Whether the poll fallback is active |
| `CheckgateStatus status` | `state` (`CheckgateConnectionState`: `idle`, `connecting`, `streaming`, `reconnecting`, `polling`, `authFailed`, `closed`), `lastSyncAt`, `error` |
| `void close()` | Ends the stream and polling, hands queued impressions and events to a final best-effort send, and closes the HTTP client |

`attributes` is a `Map<String, String>` used by targeting rules, for example
`{'plan': 'pro', 'country': 'US'}`.

For experiments, pass the same `userKey` to `track()` as to the evaluation, so
a conversion is attributed to the variant the user saw:

```dart
final variant = flags.getVariant('checkout-button-color', 'user-123');
// later, when the user converts:
flags.track('checkout_complete', 'user-123', value: 49.99);
```

## Error handling and lifecycle

- `connect()` completes with a `CheckgateAuthException` (its `statusCode` is
  401 or 403; the message contains no key) when the server rejects the SDK
  key. Retries stop, flags already loaded keep being served, and calling
  `connect()` again retries.
- `connect()` completes with a `StateError` if `close()` is called before it
  completes.
- While the server is unreachable, `connect()` does not fail: it keeps
  retrying. Use `.timeout(...)` if the caller cannot wait.
- Evaluations never throw for an unknown flag or a missing connection; they
  return `false`, `null` or `defaultValue`.
- Network, parse and storage errors, failed impression or event reports, and
  exceptions from `onChange` callbacks are swallowed; they never reach the
  caller.
- `close()` resets readiness: evaluations return `false` / `null` until the
  next `connect()`, which reconnects the same client.
- Impressions and `track()` events are only queued after the stream's
  `connected` event has named the environment; earlier ones are dropped.
  Reports are best-effort, at most 500 per request, and at most 10,000 are
  buffered (the newest are kept).

## Linking the native engine

The Dart side opens the engine like this:

| Platform | How the engine is found | Verified for 0.1.31 |
|---|---|---|
| iOS (device, simulator) | static library linked into the app (`DynamicLibrary.process()`) | iOS 17 simulator, x86_64: end to end |
| macOS | static library linked into the app, as on iOS | no |
| Android (arm64-v8a, armeabi-v7a, x86_64) | `libcheckgate_flutter.so` bundled in `jniLibs/<abi>/` | no |
| Linux | `libcheckgate_flutter.so` bundled with the app | no |
| Windows | `checkgate_flutter.dll` bundled with the app | no |

Build the engine from `sdks/flutter/dart/rust` in a clone of the repository
checked out at the tag of your package version (the crate inherits workspace
settings, so it does not build on its own from the pub cache). Cargo names the
outputs `libflutter.a`, `libflutter.so` and `flutter.dll`; rename the shared
library to the name in the table. For iOS, link the static library in whole
(`-force_load`). The step-by-step commands are in the
[Flutter SDK guide](https://checkgate-sh.github.io/checkgate/sdks/flutter#linking-the-native-engine).

Verification in a Flutter app is recorded for 0.1.31 only, as in the table.
For 0.2.0, CI runs the Dart unit tests without the engine, and the shared SSE
fault-injection suite against the Dart client with the engine built as a
Linux shared library and loaded by the Dart VM.

## Platform compatibility

- Dart SDK `>=3.0.0 <4.0.0`, Flutter `>=3.10.0`.
- Flutter web is not supported: the client uses `dart:ffi`.
- Mobile and desktop platforms: see the table above; only the iOS simulator
  was verified end to end (for 0.1.31).

## Security considerations

- The SDK key ships inside your app, so treat it as visible to its users. It
  only reaches the SDK endpoints of its environment (stream, flag reads,
  impression and event reports). Use an `https://` server URL; the key is
  sent as an `Authorization: Bearer` header.
- Impressions contain the flag key, the user key you pass, the resolved value
  and the evaluation time. Attributes are sent only with
  `sendEvaluationContext: true`. Set `reportImpressions: false` to send none.
- A `storage` adapter receives the raw flag configuration (rules, values),
  under the key `checkgate:flags:<serverUrl>`. Choose storage accordingly.

## Documentation

- Flutter SDK guide: https://checkgate-sh.github.io/checkgate/sdks/flutter
- All SDKs: https://checkgate-sh.github.io/checkgate/sdks/
- Changelog: https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md
- Source: https://github.com/checkgate-sh/checkgate/tree/main/sdks/flutter/dart

## License

Apache-2.0
