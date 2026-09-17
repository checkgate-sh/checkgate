---
title: "Type-Safe Schema CLI — Generate flag types for TS/Dart/Rust"
description: "Use @checkgate/cli to generate type-safe feature-flag accessors for TypeScript, Dart, and Rust from a running Checkgate server or a local JSON export."
---

# CLI (`@checkgate/cli`)

`@checkgate/cli` generates **type-safe flag accessors** for TypeScript, Dart, and Rust directly from your Checkgate flag definitions. Instead of hard-coding flag keys as loose strings scattered across your codebase, `checkgate typegen` reads your flags — from a running server or a local JSON export — and emits a typed module, so a wrong key or a mistyped value becomes a **compile error** rather than a production surprise.

## Installation

```bash
npm install -g @checkgate/cli
# or run without installing:
npx @checkgate/cli typegen --help
```

Requires Node 18+ (it uses the built-in `fetch`) and has zero runtime dependencies.

## Usage

```bash
checkgate typegen --lang <langs> [source] [output]
```

### Read from a running server

Authenticate with a personal access token (or an SDK key):

```bash
checkgate typegen --lang ts \
  --url http://localhost:3000 \
  --env <environment-id> \
  --token <personal-access-token> \
  --out src/flags.ts
```

Connection details can also come from the environment: `CHECKGATE_URL`, `CHECKGATE_ENV`, and `CHECKGATE_TOKEN`.

### Read from a local JSON file

A local `/flags` API export needs no network — ideal for CI:

```bash
checkgate typegen --lang ts,dart,rust --input flags.json --out-dir ./generated
```

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

Given a boolean `new-dashboard`, a string `checkout-color`, an integer `max_items`, and a JSON `theme-config`, the CLI emits language-appropriate types, a defaults map, and a typed wrapper over your existing client.

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

A `FlagKey` enum, a `kFlagDefaults` map, and a `TypedFlags` wrapper with one strongly-typed getter per flag:

```dart
final flags = TypedFlags(checkgate);
final String color = flags.checkoutColor(userId);
```

### Rust

A `FlagKey` enum (`as_str()`, `ALL`, `Display`) plus a `defaults` module of typed constants — dependency-free. JSON flags' defaults are emitted as raw JSON string constants:

```rust
let key = FlagKey::CheckoutColor.as_str(); // "checkout-color"
let fallback = defaults::CHECKOUT_COLOR;   // "blue"
```

## Notes

- Output is **deterministic** — flags are sorted by key, so regenerating produces clean diffs. Regenerate in CI and check the result in, or fail the build on drift.
- **Archived flags are excluded.** Flags without a configured `default_value` fall back to a type-appropriate zero value (`false` / `""` / `0` / `null`).
- Keys that aren't valid identifiers (hyphens, leading digits) are converted safely and de-duplicated per language.

## See also

- [Core Concepts](/guide/concepts) — flag types, targeting rules, and rollouts.
- [Web SDK](/sdks/web) and [Node SDK](/sdks/nodejs) — the clients the generated wrappers wrap.
- Source: [`cli/`](https://github.com/checkgate-sh/checkgate/tree/main/cli)
