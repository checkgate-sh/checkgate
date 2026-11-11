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

Authenticate with a personal access token (or the environment's SDK key, which may read its flags):

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
| `--timeout` | | Per-request timeout in seconds, default 30 (`CHECKGATE_TIMEOUT`). |
| `--out` | `-o` | Write to a file (single language); omit to print to stdout. |
| `--out-dir` | | Write `flags.<ext>` per language (required for multiple languages). |
| `--check` | | Write nothing; exit `3` if an output file is missing or stale. Needs `--out`/`--out-dir`. |
| `--json` | | Print one JSON result object on stdout. Needs `--out`/`--out-dir`. |
| `--help` | `-h` | Show help. |
| `--version` | | Show version. |

## Use in CI: fail the build when generated files are stale

`--check` generates in memory and compares with the files on disk. It writes nothing,
and exits `3` if any file is missing or out of date:

```bash
checkgate typegen --lang ts --input flags.json -o src/flags.ts --check
```

The `// Source:` header line is ignored by the comparison, so a file generated from
the server checks clean against the same flags read from an export.

`--check`, `--json` and `--timeout` need `@checkgate/cli` 0.1.30 or later.

### Exit codes

| Code | Meaning | What to do |
|---|---|---|
| `0` | Success — files written, or `--check` found everything up to date | Nothing |
| `1` | Operational failure: network, timeout, auth, server error, invalid flag data, file I/O | Read the error; retry if it is transient (`network`, `timeout`, `server`) |
| `2` | Usage error: unknown, missing or conflicting options. Nothing was attempted | Fix the command line |
| `3` | `--check` found missing or out-of-date files | Regenerate (same command without `--check`) and commit |
| `130` / `143` | Interrupted by Ctrl+C / SIGTERM | Re-run. Writes are atomic, so no file is left half-written |

Every request to the server times out after `--timeout` seconds (default 30, or
`CHECKGATE_TIMEOUT`); a timeout is exit `1` with `error.code: "timeout"`.

### GitHub Actions

```yaml
name: Flag types
on:
  pull_request:
  push:
    branches: [main]

permissions:
  contents: read

jobs:
  flag-types:
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@v5
      - uses: actions/setup-node@v5
        with:
          node-version: 22
      - name: Generated flag types are up to date
        env:
          CHECKGATE_URL: ${{ vars.CHECKGATE_URL }}
          CHECKGATE_ENV: ${{ vars.CHECKGATE_ENV }}
          CHECKGATE_TOKEN: ${{ secrets.CHECKGATE_TOKEN }}
        run: >
          npx --yes @checkgate/cli@0.1.30 typegen --lang ts -o src/flags.ts
          --check --json > checkgate-typegen.json
      - name: Keep the report
        if: always()
        uses: actions/upload-artifact@v6
        with:
          name: checkgate-typegen
          path: checkgate-typegen.json
          retention-days: 14
```

- Use a token that can only **read** the environment: an SDK key for that environment
  is enough, and is already public in your client apps. Never echo it.
- Secrets are not passed to `pull_request` runs from forks, so the CLI stops with exit `2`
  ("Missing access token"). If you take fork PRs, commit a flags export and check against it
  with `--input flags.json` instead of `--url`.
- The step's log names the stale files (stderr); the JSON report is the artifact.
- Pin the CLI version (`@0.1.30` above) so generated output only changes when you bump it.

## JSON result (`--json`)

With `--json`, stdout carries exactly one JSON object and nothing else. Human-readable
progress and errors go to stderr. It needs `--out` or `--out-dir`, since generated code
cannot share stdout.

```json
{
  "schema_version": 1,
  "tool": { "name": "@checkgate/cli", "version": "0.1.30" },
  "command": "typegen",
  "mode": "check",
  "status": "out_of_date",
  "exit_code": 3,
  "source": { "type": "server", "location": "https://flags.example.com", "environment": "a1b2…" },
  "flag_count": 42,
  "files": [
    { "path": "src/flags.ts", "language": "typescript", "status": "out_of_date" }
  ],
  "findings": [
    {
      "id": "typegen.out_of_date",
      "severity": "error",
      "path": "src/flags.ts",
      "message": "src/flags.ts does not match the current flag definitions (42 flag(s)).",
      "action": "Run the same command without --check to regenerate it, then commit the result."
    }
  ],
  "error": null,
  "duration_ms": 184
}
```

| Field | Values |
|---|---|
| `schema_version` | `1`. Bumped only for an incompatible change; new fields may appear in any release |
| `mode` | `write` or `check` |
| `status` | `ok`, `out_of_date`, or `error` (always consistent with `exit_code`) |
| `source.type` | `file` (`location` is the `--input` path) or `server` (`location` is the URL without credentials or query) |
| `files[].status` | `written`, `unchanged` (identical file left untouched), `up_to_date`, `out_of_date`, `missing` |
| `findings[].id` | `typegen.out_of_date`, `typegen.missing`, `flags.invalid_default` (also has `flag`) |
| `error.code` | `usage`, `invalid_input`, `auth`, `not_found`, `network`, `timeout`, `server`, `bad_response`, `io`, `internal`; `error.hint` suggests a fix when there is one |
| `duration_ms` | Wall time. The only field that differs between identical runs |

Tokens never appear in the report, in errors, or in generated files.

## For coding agents and scripts

Nothing here is specific to one agent: any tool that runs a command and reads JSON can
drive it, including Claude Code, Codex, or a shell script.

1. **Invoke** with `--json` and an explicit output: `checkgate typegen --lang ts -o src/flags.ts --json`
   (add `--check` to verify without writing).
2. **Branch on the exit code first.** `0` is done, `3` means files need regenerating, `2` means
   the command itself is wrong, and `1` is an execution failure, not a verification result.
3. **Parse stdout** as one JSON object. On `1`/`2`, read `error.code` and `error.hint`; on `3`,
   each entry in `findings` has the `path` and the `action` to take.
4. **Remediate** only from what the report says. For `typegen.*` findings that means re-running
   without `--check`; for `flags.invalid_default`, the flag's default must be fixed in Checkgate
   (or the export), which is not a code change.
5. **Re-run** the same `--check --json` command to confirm `status: "ok"` before reporting success.

`timeout`, `network` and `server` errors are transient and worth one retry. `auth`, `not_found`
and `usage` errors won't change on retry.

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

- Output is **deterministic** — flags are sorted by key, so regenerating produces clean diffs. Regenerate in CI and check the result in, or fail the build on drift with [`--check`](#use-in-ci-fail-the-build-when-generated-files-are-stale).
- Unknown or conflicting options are rejected (exit `2`) instead of being ignored.
- A flag whose `default_value` does not match its type (a string on an integer flag, an integer beyond ±2^53) stops generation with an error naming the flag. Output from it would not compile, or would differ from the server. A `null` default on a non-`json` flag counts as no default.
- Files are written atomically, and a file whose content is unchanged is left untouched.
- **Archived flags are excluded.** Flags without a configured `default_value` fall back to a type-appropriate zero value (`false` / `""` / `0` / `null`).
- Keys that aren't valid identifiers (hyphens, leading digits) are converted safely and de-duplicated per language.

## See also

- [Core Concepts](/guide/concepts) — flag types, targeting rules, and rollouts.
- [Web SDK](/sdks/web) and [Node SDK](/sdks/nodejs) — the clients the generated wrappers wrap.
- Source: [`cli/`](https://github.com/checkgate-sh/checkgate/tree/main/cli)
