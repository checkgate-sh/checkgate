#!/usr/bin/env bash
# Live end-to-end run of every SDK (sdks/conformance/live.mjs) against a real
# server on a fresh database.
#
#   DATABASE_URL=postgres://…/fresh_db REDIS_URL=redis://… scripts/sdk-live-e2e.sh [sdk|all]
#
# Builds the server (debug) and, where missing, the Node addon, the web WASM
# package and the Flutter engine for the host. The database must be empty: the
# run performs first-time setup.
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
: "${DATABASE_URL:?set DATABASE_URL to an empty database}"
: "${REDIS_URL:?set REDIS_URL}"
WHICH="${1:-all}"
PORT="${PORT:-3197}"
SETUP_TOKEN="live-e2e-$(openssl rand -hex 12)"

cargo build -q -p server
if [ "$WHICH" = all ] || [ "$WHICH" = node ]; then
  ls sdks/nodejs/*.node >/dev/null 2>&1 || (cd sdks/nodejs && npm install && npm run build:debug)
fi
if [ "$WHICH" = all ] || [ "$WHICH" = web ] || [ "$WHICH" = react-native ]; then
  (cd sdks/web && npm install --silent && npm run build >/dev/null)
  (cd sdks/react-native && npm install --silent)
fi
if [ "$WHICH" = all ] || [ "$WHICH" = flutter ]; then
  cargo build -q -p flutter
  (cd sdks/flutter/dart && flutter pub get >/dev/null)
  lib="$ROOT/target/debug/libflutter.so"; [ -f "$lib" ] || lib="$ROOT/target/debug/libflutter.dylib"
  export CHECKGATE_FLUTTER_LIB="$lib"
fi
if [ "$WHICH" = all ] || [ "$WHICH" = python ]; then
  # The adapter uses $PYTHON, else sdks/python/.venv; the package must be installed there.
  py="${PYTHON:-$ROOT/sdks/python/.venv/bin/python}"
  "$py" -c "import checkgate" 2>/dev/null || {
    echo "Python SDK not installed in $py: (cd sdks/python && maturin develop --release)" >&2; exit 1; }
fi

LOG="$(mktemp)"
SESSION_SECRET=live-e2e-0123456789-0123456789-0123456789-0123456789 \
  SETUP_TOKEN="$SETUP_TOKEN" COOKIE_SECURE=false PORT="$PORT" PUBLIC_DIR="$(mktemp -d)" \
  SSE_HEARTBEAT_INTERVAL_SECS=1 RATE_LIMIT_SDK_PER_MINUTE=0 RATE_LIMIT_SSE_PER_MINUTE=0 \
  RATE_LIMIT_API_PER_MINUTE=0 RUST_LOG=warn DATABASE_URL="$DATABASE_URL" REDIS_URL="$REDIS_URL" \
  ./target/debug/server > "$LOG" 2>&1 &
SRV=$!
trap 'kill $SRV 2>/dev/null || true' EXIT
for _ in $(seq 1 60); do curl -sf "http://127.0.0.1:$PORT/health" >/dev/null && break; sleep 1; done

rc=0
SESSION="$(mktemp)"; rm -f "$SESSION"
CHECKGATE_URL="http://127.0.0.1:$PORT" CHECKGATE_SETUP_TOKEN="$SETUP_TOKEN" CHECKGATE_E2E_SESSION="$SESSION" \
  node sdks/conformance/live.mjs "$WHICH" || rc=$?
if [ "$rc" -ne 0 ]; then echo "=== server log (tail) ==="; tail -50 "$LOG"; fi

# Server restart (SIGKILL) with each SDK connected: a second instance on the
# same database, started and killed by the check itself.
if [ "$WHICH" = all ]; then SDKS="node web react-native flutter python"; else SDKS="$WHICH"; fi
for sdk in $SDKS; do
  CHECKGATE_SERVER_BIN="$ROOT/target/debug/server" CHECKGATE_E2E_SESSION="$SESSION" \
    RESTART_PORT=$((PORT + 1)) DATABASE_URL="$DATABASE_URL" REDIS_URL="$REDIS_URL" \
    node sdks/conformance/live-restart.mjs "$sdk" || rc=1
done
exit "$rc"
