#!/usr/bin/env bash
# A/B of GET /flags/snapshot between two server builds on one host: same
# PostgreSQL and Redis containers, same seed (bench/public/sdk/seed.mjs),
# alternating runs, release builds, no CPU quota.
#
#   BASE_BIN=/path/to/old/server CAND_BIN=target/release/server \
#     bench/ab/snapshot-ab.sh
#
# Needs: a PostgreSQL container ($PG_CONTAINER, published on $PG_PORT, user
# postgres/postgres) and a Redis container ($REDIS_CONTAINER on $REDIS_PORT),
# `cargo build --release -p server --bin server --example throughput`, Node 22.
# Writes runs.jsonl (server CPU seconds, RSS, host load) and the throughput
# example's JSON per run to $OUT. Server CPU is read with `ps` (works on
# macOS and Linux); it covers warm-up plus the measured window.
set -uo pipefail
REPO="$(git rev-parse --show-toplevel)"
BASE_BIN="${BASE_BIN:?set BASE_BIN to the baseline server binary}"
CAND_BIN="${CAND_BIN:-$REPO/target/release/server}"
TP="${TP:-$REPO/target/release/examples/throughput}"
PG_CONTAINER="${PG_CONTAINER:-cg-gate-pg}"; PG_PORT="${PG_PORT:-55432}"
REDIS_CONTAINER="${REDIS_CONTAINER:-cg-gate-redis}"; REDIS_PORT="${REDIS_PORT:-56379}"
OUT="${OUT:-$REPO/bench/ab/out/snapshot}"; mkdir -p "$OUT"
ROUNDS=${ROUNDS:-3}; CONC=${CONC:-10}; SECS=${SECS:-20}; PROFILE=${PROFILE:-0}
cpu_secs() { ps -o cputime= -p "$1" | awk -F'[:.]' '{ if (NF==4) print $1*3600+$2*60+$3+$4/100; else print $1*60+$2+$3/100 }'; }
run() {
  local label=$1 bin=$2 round=$3 db="cg_ab_${1}_${3}"
  docker exec "$PG_CONTAINER" psql -U postgres -qc "DROP DATABASE IF EXISTS $db WITH (FORCE)" -c "CREATE DATABASE $db" >/dev/null
  docker exec "$REDIS_CONTAINER" redis-cli FLUSHALL >/dev/null
  local pub; pub=$(mktemp -d)
  DATABASE_URL=postgres://postgres:postgres@localhost:$PG_PORT/$db REDIS_URL=redis://localhost:$REDIS_PORT \
    PORT=3301 SESSION_SECRET=bench-session-secret-0123456789-0123456789-0123 \
    SETUP_TOKEN=bench-setup-token-0123456789 COOKIE_SECURE=false RUST_LOG=warn PUBLIC_DIR=$pub \
    RATE_LIMIT_AUTH_PER_MINUTE=0 RATE_LIMIT_API_PER_MINUTE=0 RATE_LIMIT_SDK_PER_MINUTE=0 \
    RATE_LIMIT_SSE_PER_MINUTE=0 RATE_LIMIT_INGEST_PER_MINUTE=0 \
    "$bin" > "$OUT/server-$label-$round.log" 2>&1 &
  local pid=$!
  node "$REPO/bench/public/sdk/seed.mjs" --url http://127.0.0.1:3301 \
    --setup-token bench-setup-token-0123456789 --out "$OUT/session-$label-$round.json" > /dev/null || { echo "seed failed $label"; kill $pid; return; }
  local c0; c0=$(cpu_secs $pid)
  if [ "$PROFILE" = 1 ]; then ( sleep 8; /usr/bin/sample $pid 8 -file "$OUT/sample-$label.txt" >/dev/null 2>&1 ) & fi
  "$TP" --url http://127.0.0.1:3301 --session "$OUT/session-$label-$round.json" \
    --concurrency "$CONC" --warmup-secs 5 --secs "$SECS" --server-pid $pid \
    --out "$OUT/tp-$label-$round.json" > /dev/null 2>&1 || echo "throughput failed $label $round"
  local c1; c1=$(cpu_secs $pid)
  local rss; rss=$(ps -o rss= -p $pid)
  echo "{\"label\":\"$label\",\"round\":$round,\"server_cpu_s\":$(echo "$c1 - $c0" | bc),\"rss_kib_end\":$rss,\"load\":\"$(sysctl -n vm.loadavg)\"}" >> "$OUT/runs.jsonl"
  kill -TERM $pid; wait $pid 2>/dev/null
  rm -rf "$pub"
}
for r in $(seq 1 "$ROUNDS"); do run base "$BASE_BIN" $r; run cand "$CAND_BIN" $r; done
echo "done"
