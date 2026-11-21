#!/usr/bin/env bash
# Measures the quickstart from a clean state (no quickstart data, no
# node_modules): start, setup, SDK install, first evaluation, live change.
# Prints one JSON line of timings. Deletes the quickstart data when done.
set -euo pipefail
cd "$(dirname "$0")"
ms() { python3 -c 'import time; print(int(time.time()*1000))' 2>/dev/null || node -e 'console.log(Date.now())'; }
./start.sh --down >/dev/null 2>&1 || true
rm -rf node/node_modules node/package-lock.json
export QUICKSTART_ADMIN_PASSWORD="smoke-$(openssl rand -hex 8)"

t0=$(ms)
./start.sh > /dev/null
t1=$(ms)
CHECKGATE_URL="http://localhost:${CHECKGATE_PORT:-3000}" bash -c 'set -a; . ./.env; set +a; node seed.mjs' > /dev/null
t2=$(ms)
(cd node && npm install --silent --no-audit --no-fund >/dev/null)
t3=$(ms)
set -a; . ./.env; set +a
result=$(node smoke.mjs 2>&1 >/dev/null | tail -1)
t4=$(ms)
[ "${QUICKSTART_KEEP:-}" = "1" ] || ./start.sh --down >/dev/null
echo "{\"start_to_ready_ms\":$((t1-t0)),\"setup_and_flag_ms\":$((t2-t1)),\"sdk_install_ms\":$((t3-t2)),\"connect_evaluate_change_ms\":$((t4-t3)),\"total_ms\":$((t4-t0)),\"sdk\":$result}"
