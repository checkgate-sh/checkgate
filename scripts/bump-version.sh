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
# The [workspace.package] version is the only line-initial `version = "..."` in
# Cargo.toml (members inherit it with version.workspace).
SED_INPLACE "s/^version = \".*\"$/version = \"$VERSION\"/" "$ROOT/Cargo.toml"
echo "  updated Cargo.toml [workspace.package]"

# Path dependencies on a workspace crate pin the release version exactly: a
# `^0.1.0` requirement stops matching the path crate at 0.2.0 and the
# workspace no longer resolves.
for member in $(sed -n '/^members = \[/,/^\]/p' "$ROOT/Cargo.toml" | grep -o '"[^"]*"' | tr -d '"'); do
  FILE="$ROOT/$member/Cargo.toml"
  SED_INPLACE -E "s/^(checkgate-core = \{ version = \")[^\"]*(\")/\1$VERSION\2/" "$FILE"
done
echo "  updated workspace path-dependency requirements on checkgate-core"

# Cargo.lock: only the workspace crates' own entries.
if command -v cargo >/dev/null 2>&1; then
  (cd "$ROOT" && cargo update --workspace --offline >/dev/null 2>&1 || cargo update --workspace >/dev/null)
  echo "  updated Cargo.lock (workspace crates)"
else
  echo "  WARNING: cargo not found; run \`cargo update --workspace\` before committing" >&2
fi

# --- npm packages ---
# Every publishable package plus the lockstep private ones, discovered by
# scripts/check-versions.mjs (a package added later is bumped too). Only the
# package's own version changes, in package.json and in its lockfile (top level
# and packages[""]): a sed over a lockfile would rewrite every dependency.
for dir in $(node "$ROOT/scripts/check-versions.mjs" --list npm); do
  node -e '
    const fs = require("fs");
    const [dir, version] = process.argv.slice(1);
    const edit = (file, fn) => {
      if (!fs.existsSync(file)) return false;
      const json = JSON.parse(fs.readFileSync(file, "utf8"));
      fn(json);
      fs.writeFileSync(file, JSON.stringify(json, null, 2) + "\n");
      return true;
    };
    // package.json: replace the version string in place, keeping the
    // existing formatting (escapes, inline arrays).
    const pj = dir + "/package.json";
    const text = fs.readFileSync(pj, "utf8");
    const next = text.replace(/^(  "version":\s*")[^"]*(")/m, `$1${version}$2`);
    if (JSON.parse(next).version !== version) throw new Error(pj + ": no top-level version to bump");
    fs.writeFileSync(pj, next);
    edit(dir + "/package-lock.json", (l) => {
      l.version = version;
      if (l.packages && l.packages[""]) l.packages[""].version = version;
    });
  ' "$ROOT/$dir" "$VERSION"
  echo "  updated $dir/package.json (and package-lock.json if present)"
done

# --- Python SDK (the wheel's version, and the client's __version__) ---
SED_INPLACE "s/^version = \".*\"$/version = \"$VERSION\"/" "$ROOT/sdks/python/pyproject.toml"
SED_INPLACE "s/^__version__ = \".*\"$/__version__ = \"$VERSION\"/" "$ROOT/sdks/python/python/checkgate/_client.py"
echo "  updated sdks/python/pyproject.toml and checkgate/_client.py __version__"

# --- Flutter / Dart ---
SED_INPLACE "s/^version: .*/version: $VERSION/" "$ROOT/sdks/flutter/dart/pubspec.yaml"
echo "  updated sdks/flutter/dart/pubspec.yaml"

# --- Flutter CHANGELOG (pub.dev shows it): prepend a placeholder entry unless
# the version already has one. Replace the placeholder with real notes.
CHANGELOG="$ROOT/sdks/flutter/dart/CHANGELOG.md"
TODAY="$(date +%Y-%m-%d)"
if ! grep -q "^## $VERSION" "$CHANGELOG" 2>/dev/null; then
  TMP="$(mktemp "$CHANGELOG.XXXXXX")"
  { printf '## %s (%s)\n\n- Release %s. See https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md\n\n' "$VERSION" "$TODAY" "$VERSION"; cat "$CHANGELOG"; } > "$TMP"
  mv "$TMP" "$CHANGELOG"
  echo "  prepended a placeholder $VERSION entry to sdks/flutter/dart/CHANGELOG.md (replace it with real notes)"
fi

# --- Verify ---
echo ""
node "$ROOT/scripts/check-versions.mjs" || {
  echo "" >&2
  echo "Version check failed after the bump (above). Fix the listed files." >&2
  exit 1
}

echo ""
echo "Done. All packages are now at $VERSION"
echo ""
echo "Next steps:"
echo "  1. Review changes: git diff"
echo "  2. Commit: git commit -am \"chore: bump version to $VERSION\""
echo "  3. Tag only after the release PR is merged into main: git tag -a v$VERSION"
