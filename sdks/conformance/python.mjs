// Runs the shared fault-injection suite against the Python SDK's client,
// through sdks/python/tool/conformance_adapter.py (one adapter process per
// scenario), with the real Rust engine (the PyO3 extension module).
//
//   node --test sdks/conformance/python.mjs                         # CI seeds
//   CONFORMANCE_SEEDS=1-500 node --test sdks/conformance/python.mjs # extended
//   PYTHON=path/to/python …                                         # interpreter
//
// Needs the package installed in that interpreter first:
//   (cd sdks/python && maturin develop --release)   # into sdks/python/.venv
// PYTHON defaults to sdks/python/.venv/bin/python if it exists, else python3.

import { test } from 'node:test';
import { CI_SEEDS, NAMED, generateScenario, runScenario, seedsFromEnv } from './scenario.mjs';
import { pythonAdapter } from './adapters.mjs';

const adapter = pythonAdapter();

const TIMEOUT = 60_000;
for (const [name, scenario] of Object.entries(NAMED)) {
    test(`[python] ${name}`, { timeout: TIMEOUT }, async () => {
        await runScenario(adapter, { seed: 0, ...scenario }, { label: `python-${name}` });
    });
}
for (const seed of seedsFromEnv(CI_SEEDS)) {
    test(`[python] random seed ${seed}`, { timeout: TIMEOUT }, async () => {
        await runScenario(adapter, generateScenario(seed), { label: `python-seed-${seed}` });
    });
}
