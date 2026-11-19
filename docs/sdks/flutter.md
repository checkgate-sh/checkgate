---
title: "Flutter SDK (FFI) — Feature Flags for Flutter & Dart via Dart FFI"
description: "Use the Checkgate Flutter SDK to evaluate feature flags in-process via Dart FFI — synchronous, no network call per evaluation. Works with Provider and Riverpod. You build and link the Rust engine yourself; verified on the iOS simulator for 0.1.31."
---

# Flutter SDK (FFI)

The Flutter SDK uses **Dart FFI** to call into the Rust evaluation core compiled as a native shared library. Evaluation is synchronous and happens on the Dart side with zero async overhead.

## Installation

Add to your `pubspec.yaml`:

```yaml
dependencies:
  checkgate_flutter: ^0.1.31
```

Then run:

```bash
flutter pub get
```

::: warning Native engine required
The package contains the Dart client but **no compiled engine**. Before the client can evaluate
flags you must build the Rust engine and link it into your app: see
[Linking the native engine](#linking-the-native-engine).
:::

## Quick Start

```dart
import 'package:checkgate_flutter/checkgate_flutter.dart';

void main() async {
  WidgetsFlutterBinding.ensureInitialized();

  final flags = CheckgateClient(
    serverUrl: 'https://flags.yourcompany.com',
    sdkKey: 'your-sdk-key',
  );

  await flags.connect();

  // Pass the client down however your app shares objects
  // (constructor, InheritedWidget, Provider, Riverpod — see below).
  runApp(MyApp(flags: flags));
}
```

## API Reference

### `CheckgateClient`

```dart
final client = CheckgateClient(
  serverUrl: String,     // required — base URL of your Checkgate server
  sdkKey: String,                                   // required
  reconnectBaseDelay: Duration(seconds: 2),         // optional, doubles per attempt
  maxReconnectDelay: Duration(seconds: 30),         // optional
  connectTimeout: Duration(seconds: 30),            // optional: retry an attempt without `connected`
  streamIdleTimeout: null,                          // optional: default 3 × the server's heartbeat
);
```

### `client.connect(): Future<void>`

Connects to the SSE stream and waits for the initial flag bootstrap to complete. Call this before calling `isEnabled()`.

### Connection health

`client.status` returns a `CheckgateStatus` with `state` (`CheckgateConnectionState.connecting`,
`streaming`, `reconnecting`, `polling`, `authFailed`, `closed`), `lastSyncAt` (when the client last
confirmed its flags are current: a bootstrap, a poll, or any frame on a live stream, heartbeats
included) and `error`. Flags are always served from the last known state. A stream that receives
no complete frame for 3 × the server's heartbeat (45 s by default; servers before 0.1.31 send a
keep-alive comment every 15 s, which counts) is dropped and reconnected.

A rejected key (401/403) is not retried: `connect()` completes with a `CheckgateAuthException`
(`statusCode`; the key is never in the message), the last flags keep being served, and calling
`connect()` again retries. `connect()` completes with a `StateError` if `close()` is called first.

### `client.isEnabled(flagKey, userKey, attributes): bool`

Synchronous flag evaluation via FFI. No `async/await` needed.

```dart
final enabled = client.isEnabled(
  'new-onboarding',
  userId,
  {'plan': 'pro', 'country': 'US'},
);
```

### `client.close(): void`

Closes the SSE connection, flushes buffered impressions and events, and resets readiness:
evaluations return `false` until the next `connect()`.

## Usage with Provider

```dart
// main.dart
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:checkgate_flutter/checkgate_flutter.dart';

void main() async {
  WidgetsFlutterBinding.ensureInitialized();

  final flags = CheckgateClient(
    serverUrl: const String.fromEnvironment('CHECKGATE_URL'),
    sdkKey: const String.fromEnvironment('CHECKGATE_KEY'),
  );
  await flags.connect();

  runApp(
    Provider<CheckgateClient>.value(
      value: flags,
      child: const MyApp(),
    ),
  );
}
```

```dart
// feature_screen.dart
import 'package:provider/provider.dart';
import 'package:checkgate_flutter/checkgate_flutter.dart';

class FeatureScreen extends StatelessWidget {
  const FeatureScreen({super.key, required this.user});
  final User user;

  @override
  Widget build(BuildContext context) {
    final flags = context.read<CheckgateClient>();
    final showNewUI = flags.isEnabled(
      'new-feature-screen',
      user.id,
      {'plan': user.plan},
    );

    return showNewUI ? const NewFeatureScreen() : const LegacyFeatureScreen();
  }
}
```

## Usage with Riverpod

```dart
// providers.dart
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:checkgate_flutter/checkgate_flutter.dart';

final checkgateProvider = Provider<CheckgateClient>((ref) {
  throw UnimplementedError('Override in ProviderScope');
});

// main.dart
void main() async {
  WidgetsFlutterBinding.ensureInitialized();

  final flags = CheckgateClient(serverUrl: '...', sdkKey: '...');
  await flags.connect();

  runApp(
    ProviderScope(
      overrides: [
        checkgateProvider.overrideWithValue(flags),
      ],
      child: const MyApp(),
    ),
  );
}

// In a widget
final flags = ref.read(checkgateProvider);
final enabled = flags.isEnabled('my-flag', userId, {});
```

## Platform Support

| Platform | Engine | Verified for v0.1.31 |
|----------|--------|----------------------|
| iOS (device, simulator) | static library, linked by you (see below) | iOS 17 simulator, x86_64: end to end |
| Android (arm64-v8a, armeabi-v7a, x86_64) | `libcheckgate_flutter.so`, bundled by you | no |
| macOS | static library, linked into the app like iOS | no (the Dart client's tests run on a macOS host) |
| Linux, Windows | `libcheckgate_flutter.so` / `checkgate_flutter.dll`, bundled by you | no |

## Linking the native engine

The package contains the Dart client and the Rust engine's source, but no
compiled library and no Flutter plugin that builds one. Build the engine and
link it into your app yourself (planned: a plugin that does this, as the React
Native SDK's podspec does).

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
# from sdks/flutter/dart/rust in a clone of the repository, checked out at the
# tag matching your package version (the crate inherits workspace settings, so
# it does not build on its own from the pub cache)
cargo rustc --release --lib --crate-type staticlib --target aarch64-apple-ios       # device
cargo rustc --release --lib --crate-type staticlib --target aarch64-apple-ios-sim   # simulator, Apple silicon
cargo rustc --release --lib --crate-type staticlib --target x86_64-apple-ios        # simulator, Intel
```

**iOS.** The Dart side looks the engine up in the app's own process image
(`DynamicLibrary.process()`), so the static library must be linked in whole.
Copy `target/<target>/release/libflutter.a` into `ios/` and add to
`ios/Flutter/Debug.xcconfig` and `Release.xcconfig`:

```
OTHER_LDFLAGS=$(inherited) -force_load $(PROJECT_DIR)/libflutter.a
```

(use `lipo -create` to combine the simulator architectures you need). Verified
for v0.1.31 on an iOS 17 simulator (x86_64): connect, live updates, targeting
evaluated by the linked engine, invalid-key handling.

**Android.** Build with `cargo ndk` for each ABI and place the library in
`android/app/src/main/jniLibs/<abi>/` **renamed to `libcheckgate_flutter.so`**,
the name the Dart side opens (cargo names it `libflutter.so`). Not verified on
a device or emulator for v0.1.31.

**macOS.** Like iOS: the engine is looked up in the process image, so link the static library
into the app (`-force_load`). **Linux / Windows.** Bundle the shared library renamed to
`libcheckgate_flutter.so` / `checkgate_flutter.dll` (cargo names it `libflutter.so` /
`flutter.dll`). None of these was verified for v0.1.31.
