#!/usr/bin/env bash
# Fails if generated, machine-specific or internal-only files are tracked by Git.
#
# Dependencies, build output and tool caches are recreated by each package's
# own install/build step (npm/bun install, flutter pub get, cargo build), so
# committing them only bloats the repository — and can leak into published
# packages (pub and npm both pack from the working tree).
#
#   scripts/check-tracked-artifacts.sh
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# Directories that are always generated, wherever they appear.
dirs='node_modules|\.dart_tool|target|coverage|\.gradle|\.cxx|Pods|xcuserdata|__pycache__'
pattern="(^|/)(${dirs})/"
# Generated outputs at known locations.
pattern+='|^sdks/flutter/dart/build/|^sdks/react-native/android/build/|^sdks/react-native/rust-core/'
pattern+='|^docs/\.vitepress/dist/|^server/public/'
# Generated or local-only files.
pattern+='|\.(node|tsbuildinfo|dill|pyc)$|(^|/)native-binding\.(js|d\.ts)$'
pattern+='|(^|/)\.DS_Store$|(^|/)checkgate-setup-token$|(^|/)\.env$'
# Internal reports and working notes stay local.
pattern+='|^internal_docs/'

offending="$(git ls-files | grep -E "$pattern" || true)"
if [ -n "$offending" ]; then
  count="$(wc -l <<<"$offending")"
  echo "error: $count generated or machine-specific file(s) are tracked:" >&2
  head -20 <<<"$offending" >&2
  [ "$count" -le 20 ] || echo "  … and $((count - 20)) more" >&2
  echo "Remove them with 'git rm -r --cached <path>' and make sure .gitignore covers them." >&2
  exit 1
fi
echo "ok: no generated or machine-specific files are tracked"
