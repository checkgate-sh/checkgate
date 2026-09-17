<p align="center">
  <img src="../../assets/checkgate_logo.png" width="120" alt="Checkgate Logo">
</p>

# checkgate_flutter

**Checkgate Flutter SDK (FFI)** — The official high-performance Dart client for Checkgate.
Built fundamentally on top of Dart FFI (Foreign Function Interface), this SDK bypasses traditional asynchronous platform channels to access the Rust core evaluated completely synchronously in Dart memory.

## Installation

```bash
flutter pub add checkgate_flutter
```

## Quick Start

Initialize the `CheckgateClient` instance inside your main Dart entrypoint:

```dart
import 'package:checkgate_flutter/checkgate_flutter.dart';

void main() async {
  // Ensure Flutter engine is fully initialized natively 
  WidgetsFlutterBinding.ensureInitialized();

  final checkgate = CheckgateClient(
    url: 'https://checkgate.your-company.com',
    clientKey: 'pk_mobile_xxxxxx',
  );

  // Bind to the Checkgate SSE stream to subscribe to global flag updates
  await checkgate.connect();

  runApp(MyApp(checkgate: checkgate));
}

class MyApp extends StatelessWidget {
  final CheckgateClient checkgate;
  
  const MyApp({Key? key, required this.checkgate}) : super(key: key);

  @override
  Widget build(BuildContext context) {
    // Synchronously evaluate flag targeting parameters locally!
    final showMaterial3 = checkgate.isEnabled('material-3-design', {
      'userId': '12345'
    });

    return MaterialApp(
      theme: showMaterial3 ? ThemeData.useMaterial3() : ThemeData.light(),
      home: Scaffold(...)
    );
  }
}
```

## A/B testing: track conversions

Record goal events with `track()` to measure how each flag variant converts. Pair
a flag with a goal event in an **Experiment** in the dashboard to see per-variant
conversion rates and statistical significance.

```dart
final variant = checkgate.getVariant('checkout-button-color', 'user_123');

// …later, when the user converts:
checkgate.track('checkout_complete', 'user_123', value: 49.99);
```

Events are buffered and reported asynchronously (best-effort), just like impressions.
Use the **same `userKey`** you pass to `getVariant()`/`isEnabled()` so conversions
attribute to the right variant.

## Why Checkgate FFI?
* **Synchronous FFI Evaluation:** You do not need to `await` flag resolution ever. The evaluation runs in Rust natively in sub-microseconds avoiding Dart async blocking limits.
* **Instant Propagation (0 to <50ms):** Leverages server-side SSE channels natively within the Rust binary so flag adjustments happen seamlessly within your app flow.

Review advanced patterns inside the [official Checkgate documentation](https://checkgate-sh.github.io/checkgate).
