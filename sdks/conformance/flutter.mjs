// Runs the shared fault-injection suite against the Flutter SDK's Dart client,
// through tool/conformance_adapter.dart (one adapter process per scenario).
//
//   node --test sdks/conformance/flutter.mjs                       # CI seeds
//   CONFORMANCE_SEEDS=1-500 node --test sdks/conformance/flutter.mjs   # extended
//   CHECKGATE_FLUTTER_LIB=target/debug/libflutter.dylib …          # real engine
//
// Needs `flutter pub get` in sdks/flutter/dart first. DART defaults to `dart`.

import { test } from 'node:test';
import { CI_SEEDS, NAMED, generateScenario, runScenario, seedsFromEnv } from './scenario.mjs';
import { flutterAdapter } from './adapters.mjs';

const adapter = flutterAdapter();

const TIMEOUT = 60_000;
for (const [name, scenario] of Object.entries(NAMED)) {
    test(`[flutter] ${name}`, { timeout: TIMEOUT }, async () => {
        await runScenario(adapter, { seed: 0, ...scenario }, { label: `flutter-${name}` });
    });
}
for (const seed of seedsFromEnv(CI_SEEDS)) {
    test(`[flutter] random seed ${seed}`, { timeout: TIMEOUT }, async () => {
        await runScenario(adapter, generateScenario(seed), { label: `flutter-seed-${seed}` });
    });
}
