<div align="center">
  <img src="../assets/checkgate_logo.png" alt="Checkgate" width="72" />
  <h1>@checkgate/cli</h1>
  <p>Generate <strong>type-safe flag accessors</strong> for TypeScript, Dart, and Rust from your Checkgate flag definitions.</p>
</div>

Stop hard-coding flag keys as loose strings. `checkgate typegen` reads your flag
definitions — straight from a running server or a local JSON export — and emits a
typed module so a wrong key or a mis-typed value is a **compile error**, not a
production surprise.

## Installation

```bash
npm install -g @checkgate/cli
# or run without installing:
npx @checkgate/cli typegen --help
```

Requires Node 18+ (uses the built-in `fetch`). Zero runtime dependencies.

## Usage

```bash
checkgate typegen --lang <langs> [source] [output]
```

### Pick a source

From a running server, authenticated with a personal access token (or SDK key):

```bash
checkgate typegen --lang ts \
  --url http://localhost:3000 \
  --env <environment-id> \
  --token <personal-access-token> \
  --out src/flags.ts
```

Or from a local JSON file (e.g. a `/flags` API export), which needs no network — ideal for CI:

```bash
checkgate typegen --lang ts,dart,rust --input flags.json --out-dir ./generated
```

Server connection details can also come from the environment:
`CHECKGATE_URL`, `CHECKGATE_ENV`, `CHECKGATE_TOKEN`.

### Options

| Flag | Alias | Description |
|---|---|---|
| `--lang` | `-l` | `ts`\|`typescript`, `dart`, `rust`\|`rs`. Comma-separate for several. |
| `--input` | `-i` | Read flags from a local JSON file instead of the API. |
| `--url` | `-u` | Server base URL (`CHECKGATE_URL`). |
| `--env` | `-e` | Environment id to read (`CHECKGATE_ENV`). |
| `--token` | `-t` | Personal access token / SDK key (`CHECKGATE_TOKEN`). |
| `--out` | `-o` | Write to a file (single language); omit to print to stdout. |
| `--out-dir` | | Write `flags.<ext>` per language (required for multiple languages). |
| `--help` | `-h` | Show help. |
| `--version` | | Show version. |

## What it generates

Given a boolean `new-dashboard`, a string `checkout-color`, an integer `max_items`,
and a JSON `theme-config`:

### TypeScript

```typescript
export type FlagKey = "new-dashboard" | "checkout-color" | "max_items" | "theme-config"

export interface FlagValueTypes {
  "new-dashboard": boolean
  "checkout-color": string
  "max_items": number
  "theme-config": unknown
}

export const FLAG_DEFAULTS = { /* each flag's configured default */ }

export function typedFlags(client: CheckgateLike) { /* … */ }
```

Wrap your existing client for compile-time-checked keys and correctly-typed values:

```typescript
import { CheckgateClient } from '@checkgate/node'
import { typedFlags } from './flags'

const flags = typedFlags(new CheckgateClient({ /* … */ }))

const color = flags.getValue('checkout-color', userId) // typed as string
flags.getValue('chekout-color', userId)                // ❌ compile error — typo caught
```

### Dart

A `FlagKey` enum, a `kFlagDefaults` map, and a `TypedFlags` wrapper with one
strongly-typed getter per flag:

```dart
final flags = TypedFlags(checkgate);
final String color = flags.checkoutColor(userId);
```

### Rust

A `FlagKey` enum (`as_str()`, `ALL`, `Display`) plus a `defaults` module of typed
constants — dependency-free (`json` flags' defaults are emitted as raw JSON string
constants):

```rust
let key = FlagKey::CheckoutColor.as_str(); // "checkout-color"
let fallback = defaults::CHECKOUT_COLOR;   // "blue"
```

## Notes

- Output is deterministic — flags are sorted by key, so regenerating produces
  clean diffs. Regenerate in CI and check the result in, or fail the build on drift.
- Archived flags are excluded. Flags without a configured `default_value` fall back
  to a type-appropriate zero value (`false` / `""` / `0` / `null`).
- Keys that aren't valid identifiers (hyphens, leading digits) are converted safely
  and de-duplicated per language.

---

Part of [Checkgate](https://github.com/checkgate-sh/checkgate) — the self-hosted feature-flag platform.
