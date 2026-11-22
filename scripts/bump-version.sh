#!/usr/bin/env bash
# Usage: ./scripts/bump-version.sh <new-version>
# Updates version in all packages and SDKs. Source of truth: Cargo.toml [workspace.package].

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [ $# -ne 1 ]; then
  echo "Usage: $0 <version>" >&2
  exit 1
fi

VERSION="$1"

# Validate semver format
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9._-]+)?(\+[a-zA-Z0-9._-]+)?$ ]]; then
  echo "Error: '$VERSION' is not a valid semver string" >&2
  exit 1
fi

echo "Bumping all packages to $VERSION"

# In-place sed without a backup file: BSD sed (macOS) needs an explicit empty
# extension (`-i ''`); GNU sed (Linux) takes `-i` alone and would read '' as
# the script, failing on the first edit.
if sed --version >/dev/null 2>&1; then
  SED_INPLACE() { sed -i "$@"; }
else
  SED_INPLACE() { sed -i '' "$@"; }
fi

# --- Rust workspace (source of truth) ---
SED_INPLACE "s/^version = \".*\"$/version = \"$VERSION\"/" "$ROOT/Cargo.toml"
echo "  updated Cargo.toml [workspace.package]"

# --- JS SDKs ---
for pkg in nodejs node-integrations openfeature-provider web react-native; do
  FILE="$ROOT/sdks/$pkg/package.json"
  SED_INPLACE "s/\"version\": \".*\"/\"version\": \"$VERSION\"/" "$FILE"
  echo "  updated sdks/$pkg/package.json"
done

# --- Standalone published JS packages (each has its own release workflow) ---
for pkg in cli edge ssr; do
  FILE="$ROOT/$pkg/package.json"
  SED_INPLACE "s/\"version\": \".*\"/\"version\": \"$VERSION\"/" "$FILE"
  echo "  updated $pkg/package.json"
done

# --- MCP server (its code reads the version from package.json) ---
SED_INPLACE "s/\"version\": \".*\"/\"version\": \"$VERSION\"/" "$ROOT/integrations/mcp-server/package.json"
echo "  updated integrations/mcp-server/package.json"

# --- Python SDK (the wheel's version, and the client's __version__) ---
SED_INPLACE "s/^version = \".*\"$/version = \"$VERSION\"/" "$ROOT/sdks/python/pyproject.toml"
SED_INPLACE "s/^__version__ = \".*\"$/__version__ = \"$VERSION\"/" "$ROOT/sdks/python/python/checkgate/_client.py"
echo "  updated sdks/python/pyproject.toml and checkgate/_client.py __version__"

# --- Dashboard ---
SED_INPLACE "s/\"version\": \".*\"/\"version\": \"$VERSION\"/" "$ROOT/dashboard/package.json"
echo "  updated dashboard/package.json"

# --- npm lockfiles: only the package's own version (top level and
# packages[""]); a sed over a lockfile would rewrite every dependency's
# version too. Lockfiles left at the old version made `npm ci` trees report a
# version the package no longer has.
for dir in sdks/nodejs sdks/web sdks/react-native sdks/openfeature-provider \
           sdks/node-integrations integrations/mcp-server cli edge ssr dashboard; do
  LOCK="$ROOT/$dir/package-lock.json"
  [ -f "$LOCK" ] || continue
  node -e '
    const fs = require("fs");
    const [file, version] = process.argv.slice(1);
    const lock = JSON.parse(fs.readFileSync(file, "utf8"));
    lock.version = version;
    if (lock.packages && lock.packages[""]) lock.packages[""].version = version;
    fs.writeFileSync(file, JSON.stringify(lock, null, 2) + "\n");
  ' "$LOCK" "$VERSION"
  echo "  updated $dir/package-lock.json"
done

# --- Flutter / Dart ---
SED_INPLACE "s/^version: .*/version: $VERSION/" "$ROOT/sdks/flutter/dart/pubspec.yaml"
echo "  updated sdks/flutter/dart/pubspec.yaml"

# --- Flutter CHANGELOG (prepend new entry) ---
CHANGELOG="$ROOT/sdks/flutter/dart/CHANGELOG.md"
TODAY="$(date +%Y-%m-%d)"
ENTRY="## $VERSION ($TODAY)\n\n- Release $VERSION.\n"
# Only prepend if the version heading isn't already present.
if ! grep -q "^## $VERSION" "$CHANGELOG" 2>/dev/null; then
  printf "%b\n" "$ENTRY" | cat - "$CHANGELOG" > /tmp/_cg_changelog && mv /tmp/_cg_changelog "$CHANGELOG"
fi
echo "  prepended $VERSION entry to sdks/flutter/dart/CHANGELOG.md (edit manually to add details)"

echo ""
echo "Done. All packages are now at $VERSION"
echo ""
echo "Next steps:"
echo "  1. Review changes: git diff"
echo "  2. Commit: git commit -am \"chore: bump version to $VERSION\""
echo "  3. Tag:    git tag v$VERSION"
