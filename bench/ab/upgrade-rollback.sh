#!/usr/bin/env bash
# Upgrade / rollback / mixed-version check on one PostgreSQL database:
#   1. v0.1.30 creates the schema and data
#   2. v0.1.31 starts on it (upgrade), reads it, writes a change
#   3. v0.1.30 and v0.1.31 run together (rolling upgrade): a change written
#      through one reaches an SSE client of the other
#   4. v0.1.30 starts alone again (rollback) and serves v0.1.31's writes
#
#   OLD_BIN=/path/to/v0.1.30/server NEW_BIN=target/release/server bench/ab/upgrade-rollback.sh
#
# Same container prerequisites as snapshot-ab.sh.
set -uo pipefail
REPO="$(git rev-parse --show-toplevel)"
OLD="${OLD_BIN:?set OLD_BIN to the server binary of the previous release}"
NEW="${NEW_BIN:-$REPO/target/release/server}"
PG_CONTAINER="${PG_CONTAINER:-cg-gate-pg}"; PG_PORT="${PG_PORT:-55432}"
REDIS_CONTAINER="${REDIS_CONTAINER:-cg-gate-redis}"; REDIS_PORT="${REDIS_PORT:-56379}"
OUT="${OUT:-$REPO/bench/ab/out/upgrade}"; mkdir -p "$OUT"; DB=cg_upgrade
docker exec "$PG_CONTAINER" psql -U postgres -qc "DROP DATABASE IF EXISTS $DB WITH (FORCE)" -c "CREATE DATABASE $DB" >/dev/null
docker exec "$REDIS_CONTAINER" redis-cli FLUSHALL >/dev/null
start() { # bin port name
  DATABASE_URL=postgres://postgres:postgres@localhost:$PG_PORT/$DB REDIS_URL=redis://localhost:$REDIS_PORT PORT=$2 \
  SESSION_SECRET=bench-session-secret-0123456789-0123456789-0123 SETUP_TOKEN=bench-setup-token-0123456789 \
  COOKIE_SECURE=false RUST_LOG=warn PUBLIC_DIR=$(mktemp -d) SYNC_RECONCILE_INTERVAL_SECS=2 \
  RATE_LIMIT_API_PER_MINUTE=0 RATE_LIMIT_SDK_PER_MINUTE=0 RATE_LIMIT_AUTH_PER_MINUTE=0 RATE_LIMIT_SSE_PER_MINUTE=0 \
  "$1" > "$OUT/$3.log" 2>&1 &
  echo $!
}
ready() { for _ in $(seq 1 100); do curl -sf "http://127.0.0.1:$1/health/ready" >/dev/null && return 0; sleep 0.2; done; echo "NOT READY on $1"; return 1; }
stop() { kill -TERM "$1"; wait "$1" 2>/dev/null; }
snap() { curl -sf -H "Authorization: Bearer $KEY" "http://127.0.0.1:$1/flags/snapshot" | python3 -c "import sys,json,hashlib;d=json.load(sys.stdin);print(len(d), hashlib.sha256(json.dumps(sorted(d,key=lambda f:f['key']),sort_keys=True).encode()).hexdigest()[:16])"; }
patch() { curl -sf -o /dev/null -w "%{http_code}" -X PATCH -H "Cookie: $COOKIE" -H "X-Checkgate-Request: true" -H "Content-Type: application/json" \
  -d "{\"default_value\":\"$3\"}" "http://127.0.0.1:$1/api/environments/$ENV/flags/$2"; }

echo "== 1. v0.1.30 creates schema + 101 flags"
P=$(start "$OLD" 3311 old-1); ready 3311
node "$REPO/bench/public/sdk/seed.mjs" --url http://127.0.0.1:3311 --setup-token bench-setup-token-0123456789 --out "$OUT/session.json" >/dev/null
KEY=$(python3 -c "import json;print(json.load(open('$OUT/session.json'))['sdk_key'])")
ENV=$(python3 -c "import json;print(json.load(open('$OUT/session.json'))['environment_id'])")
COOKIE=$(python3 -c "import json;print(json.load(open('$OUT/session.json'))['cookie'])")
echo "old snapshot:        $(snap 3311)"; stop $P
MIG_BEFORE=$(docker exec "$PG_CONTAINER" psql -U postgres -d $DB -tAc "SELECT count(*), max(version) FROM _sqlx_migrations")

echo "== 2. upgrade: v0.1.31 on the same database"
P=$(start "$NEW" 3312 new-1); ready 3312 && echo "new is ready"
echo "new snapshot:        $(snap 3312)"
MIG_AFTER=$(docker exec "$PG_CONTAINER" psql -U postgres -d $DB -tAc "SELECT count(*), max(version) FROM _sqlx_migrations")
echo "migrations before/after: $MIG_BEFORE / $MIG_AFTER"
echo "patch via new: $(patch 3312 hot written-by-v0131)"

echo "== 3. rolling: v0.1.30 and v0.1.31 together"
Q=$(start "$OLD" 3311 old-2); ready 3311 && echo "old is ready next to new"
curl -sN -H "Authorization: Bearer $KEY" http://127.0.0.1:3311/stream > "$OUT/old-stream.txt" &
C1=$!
curl -sN -H "Authorization: Bearer $KEY" http://127.0.0.1:3312/stream > "$OUT/new-stream.txt" &
C2=$!
sleep 2
echo "patch via new: $(patch 3312 hot new-to-old)"; echo "patch via old: $(patch 3311 hot old-to-new)"
sleep 2; kill $C1 $C2 2>/dev/null
echo "old instance's client saw new's change: $(grep -c 'new-to-old' "$OUT/old-stream.txt")"
echo "new instance's client saw old's change: $(grep -c 'old-to-new' "$OUT/new-stream.txt")"
stop $P; stop $Q

echo "== 4. rollback: v0.1.30 alone"
P=$(start "$OLD" 3311 old-3); ready 3311 && echo "old is ready after rollback"
echo "old snapshot:        $(snap 3311)"
echo "hot after rollback:  $(curl -sf -H "Authorization: Bearer $KEY" http://127.0.0.1:3311/flags/snapshot | python3 -c "import sys,json;print([f['default_value'] for f in json.load(sys.stdin) if f['key']=='hot'])")"
stop $P
grep -h '"level":"ERROR"' $OUT/*.log | head -3
