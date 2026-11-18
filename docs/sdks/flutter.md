---
title: "Flutter SDK (FFI) — Feature Flags for Flutter & Dart via Dart FFI"
description: "Use the Checkgate Flutter SDK to evaluate feature flags in-process via Dart FFI — synchronous, no network call per evaluation. Works with Provider, Riverpod, iOS, Android, macOS, Linux, and Windows."
---

# Flutter SDK (FFI)

The Flutter SDK uses **Dart FFI** to call into the Rust evaluation core compiled as a native shared library. Evaluation is synchronous and happens on the Dart side with zero async overhead.

## Installation

Add to your `pubspec.yaml`:

```yaml
dependencies:
  checkgate_flutter: ^0.1.0
```

Then run:

```bash
flutter pub get
```

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

  runApp(
    CheckgateProvider(client: flags, child: const MyApp()),
  );
}
```

## API Reference

### `CheckgateClient`

```dart
final client = CheckgateClient(
  serverUrl: String,     // required — base URL of your Checkgate server
  sdkKey: String?,       // optional — for authenticated servers
  reconnectDelay: Duration(seconds: 3), // optional
);
```

### `client.connect(): Future<void>`

Connects to the SSE stream and waits for the initial flag bootstrap to complete. Call this before calling `isEnabled()`.

### `client.isEnabled(flagKey, userKey, attributes): bool`

Synchronous flag evaluation via FFI. No `async/await` needed.

```dart
final enabled = client.isEnabled(
  'new-onboarding',
  userId,
  {'plan': 'pro', 'country': 'US'},
);
```

### `client.disconnect(): void`

Closes the SSE connection and releases native resources.

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

  final flags = CheckgateClient(serverUrl: '...');
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
| macOS, Linux, Windows | shared library, bundled by you | host tests only (the Dart client with the engine built for the host) |

## Linking the native engine

The package contains the Dart client and the Rust engine's source, but no
compiled library and no Flutter plugin that builds one. Build the engine and
link it into your app yourself (planned: a plugin that does this, as the React
Native SDK's podspec does).

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
# from the package's rust/ directory (or sdks/flutter/dart/rust in the repo)
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

**macOS / Linux / Windows.** Not verified for v0.1.31.
