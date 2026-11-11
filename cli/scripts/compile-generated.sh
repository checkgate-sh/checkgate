#!/usr/bin/env bash
# Generate code from every fixture and compile it with each language's real
# toolchain, at its strictest: rustc -D warnings, tsc --strict, and
# dart analyze --fatal-infos. Matching on output strings cannot catch code that
# does not compile; this does.
#
# A missing toolchain is an error, never a skip — a check that silently does
# nothing is how broken output shipped before.
#
#   cli/scripts/compile-generated.sh            # needs node, rustc, dart; tsc via npx
#   TSC=/path/to/tsc cli/scripts/compile-generated.sh
set -euo pipefail

cd "$(dirname "$0")/.."
OUT="$(mktemp -d)"
trap 'rm -rf "$OUT"' EXIT

for tool in node rustc dart; do
  command -v "$tool" >/dev/null || { echo "error: $tool is required" >&2; exit 1; }
done
# shellcheck disable=SC2206 # deliberate word splitting of a command line
TSC=(${TSC:-npx --yes -p typescript@5.9.3 tsc})

for fixture in test/fixtures/*.json; do
  name="$(basename "$fixture" .json)"
  dir="$OUT/$name"
  echo "== $name"
  node bin/checkgate.js typegen --lang ts,dart,rust --input "$fixture" --out-dir "$dir"

  # Run rustc outside the repository so its rust-toolchain.toml does not apply.
  (cd "$dir" && rustc --edition 2021 --crate-type lib -D warnings flags.rs -o "$dir/libflags.rlib")
  echo "   rust ok"

  "${TSC[@]}" --strict --noEmit --target es2022 "$dir/flags.ts"
  echo "   typescript ok"

  mkdir -p "$dir/dart/lib"
  cp "$dir/flags.dart" "$dir/dart/lib/"
  printf 'name: generated_flags_check\nenvironment:\n  sdk: ">=3.0.0 <4.0.0"\n' > "$dir/dart/pubspec.yaml"
  (cd "$dir/dart" && dart analyze --fatal-infos --fatal-warnings lib >/dev/null)
  echo "   dart ok"
done
