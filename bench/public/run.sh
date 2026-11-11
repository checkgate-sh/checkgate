#!/usr/bin/env bash
# Public benchmark suite: evaluation latency, server throughput per CPU and RAM,
# and flag-update propagation through real SDKs. See bench/public/README.md.
#
#   bench/public/run.sh eval          # Rust core (Criterion) + Node SDK, on the host
#   bench/public/run.sh server        # snapshot throughput, REPS fresh stacks
#   bench/public/run.sh propagation   # SDK propagation, REPS fresh stacks
#   bench/public/run.sh environment   # environment.json only
#   bench/public/run.sh summarize     # summary.json + README graphic from the results
#   bench/public/run.sh all           # all of the above, in that order
#
# Environment (defaults in brackets):
#   OUT            results directory [docs/performance/results/v0.1.30]
#   REPS           repetitions of server and propagation [3]
#   EVAL_REPS      repetitions of the evaluation benchmarks [3]
#   CONCURRENCY    throughput client counts [10,100,500,1000]
#   WARMUP_SECS    unrecorded load before each window [10]
#   WINDOW_SECS    measurement window per concurrency level [60]
#   CLIENTS        SDK clients per propagation profile [1,100,500,1000]
#   UPDATES        flag updates per propagation profile [200]
#   BENCH_SERVER_CPUS / BENCH_SERVER_MEMORY   server limits [1 / 512m]
#
# Each repetition starts from an empty database on a freshly created stack.
# A repetition that fails keeps its output under raw/failed/ and the suite
# carries on; summarize.py reports failed runs, it never drops them silently.
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
HERE="$ROOT/bench/public"
OUT="${OUT:-$ROOT/docs/performance/results/v0.1.30}"
RAW="$OUT/raw"
REPS="${REPS:-3}"
EVAL_REPS="${EVAL_REPS:-3}"
CONCURRENCY="${CONCURRENCY:-10,100,500,1000}"
WARMUP_SECS="${WARMUP_SECS:-10}"
WINDOW_SECS="${WINDOW_SECS:-60}"
CLIENTS="${CLIENTS:-1,100,500,1000}"
UPDATES="${UPDATES:-200}"
SETUP_TOKEN="bench-setup-token-0123456789"   # matches compose.yml

export BENCH_WORK="$HERE/.work"
export BENCH_RESULTS="$RAW"
mkdir -p "$RAW/failed" "$BENCH_WORK"

compose() { docker compose -f "$HERE/compose.yml" "$@"; }

loadavg() {
  if [ -r /proc/loadavg ]; then cut -d' ' -f1-3 /proc/loadavg
  else sysctl -n vm.loadavg | tr -d '{}' | xargs; fi
}

# Host swap in use, MiB: swapping stalls everything and shows up as latency.
swap_mib() {
  if [ -r /proc/meminfo ]; then
    awk '/^SwapTotal/{t=$2} /^SwapFree/{f=$2} END{printf "%d", (t-f)/1024}' /proc/meminfo
  else sysctl -n vm.swapusage | sed -E 's/.*used = ([0-9.]+)M.*/\1/' | cut -d. -f1; fi
}

# One line per run in raw/runs.jsonl: what ran, when, host load and swap, and outcome.
log_run() { # phase rep status started load_before swap_before
  printf '{"phase":"%s","rep":%s,"status":"%s","started":"%s","ended":"%s","host_load_before":"%s","host_load_after":"%s","host_swap_used_mib_before":%s,"host_swap_used_mib_after":%s}\n' \
    "$1" "$2" "$3" "$4" "$(date -u +%FT%TZ)" "$5" "$(loadavg)" "$6" "$(swap_mib)" >> "$RAW/runs.jsonl"
}

fresh_stack() {
  compose --profile tools down -v --remove-orphans >/dev/null 2>&1 || true
  compose up -d --wait postgres redis
  compose up -d server
  compose --profile tools run --rm -T sdk seed.mjs \
    --url http://server:3000 --setup-token "$SETUP_TOKEN" --out /work/session.json
}

# Samples every container's CPU and memory once a second while a run is going.
start_stats() { docker stats --format '{{json .}}' > "$1" 2>/dev/null & STATS_PID=$!; }
stop_stats() { kill "$STATS_PID" 2>/dev/null || true; wait "$STATS_PID" 2>/dev/null || true; }

run_eval() {
  local target="$ROOT/target/bench-public"
  for r in $(seq 1 "$EVAL_REPS"); do
    local started load swap; started="$(date -u +%FT%TZ)"; load="$(loadavg)"; swap="$(swap_mib)"
    rm -rf "$target/criterion"
    if (cd "$ROOT" && CARGO_TARGET_DIR="$target" cargo bench -p checkgate-bench --bench evaluation) \
        > "$RAW/criterion-run$r.txt" 2>&1 \
       && python3 "$HERE/scripts/extract_criterion.py" "$target/criterion" "$RAW/criterion-run$r.txt" \
        > "$RAW/evaluation-rust-run$r.json"; then
      log_run evaluation-rust "$r" ok "$started" "$load" "$swap"
    else
      mv "$RAW/criterion-run$r.txt" "$RAW/failed/" 2>/dev/null || true
      rm -f "$RAW/evaluation-rust-run$r.json"
      log_run evaluation-rust "$r" failed "$started" "$load" "$swap"
    fi
  done

  if ! ls "$ROOT"/sdks/nodejs/*.node >/dev/null 2>&1; then
    (cd "$ROOT/sdks/nodejs" && npm ci && npm run build)
  fi
  for r in $(seq 1 "$EVAL_REPS"); do
    local started load swap; started="$(date -u +%FT%TZ)"; load="$(loadavg)"; swap="$(swap_mib)"
    if node "$HERE/sdk/eval.mjs" --out "$RAW/evaluation-node-run$r.json"; then
      log_run evaluation-node "$r" ok "$started" "$load" "$swap"
    else
      log_run evaluation-node "$r" failed "$started" "$load" "$swap"
    fi
  done
}

run_server() {
  compose --profile tools build
  for r in $(seq 1 "$REPS"); do
    local started load swap; started="$(date -u +%FT%TZ)"; load="$(loadavg)"; swap="$(swap_mib)"
    fresh_stack
    start_stats "$BENCH_WORK/stats.jsonl"
    if compose --profile tools run --rm -T loadgen \
        --url http://server:3000 --session /work/session.json \
        --concurrency "$CONCURRENCY" --warmup-secs "$WARMUP_SECS" --secs "$WINDOW_SECS" \
        --server-pid 1 --out "/results/throughput-run$r.json" > /dev/null; then
      stop_stats
      python3 "$HERE/scripts/docker_stats.py" "$BENCH_WORK/stats.jsonl" > "$RAW/throughput-run$r.containers.json"
      log_run throughput "$r" ok "$started" "$load" "$swap"
    else
      stop_stats
      mv "$RAW/throughput-run$r.json" "$RAW/failed/" 2>/dev/null || true
      cp "$BENCH_WORK/stats.jsonl" "$RAW/failed/throughput-run$r.stats.jsonl" || true
      compose logs server > "$RAW/failed/throughput-run$r.server.log" 2>&1 || true
      log_run throughput "$r" failed "$started" "$load" "$swap"
    fi
  done
  compose --profile tools down -v --remove-orphans
}

run_propagation() {
  compose --profile tools build
  for r in $(seq 1 "$REPS"); do
    local started load swap; started="$(date -u +%FT%TZ)"; load="$(loadavg)"; swap="$(swap_mib)"
    fresh_stack
    start_stats "$BENCH_WORK/stats.jsonl"
    if compose --profile tools run --rm -T sdk propagation.mjs \
        --session /work/session.json --clients "$CLIENTS" --updates "$UPDATES" \
        --run-tag "r$r" --out "/results/propagation-run$r.json"; then
      stop_stats
      python3 "$HERE/scripts/docker_stats.py" "$BENCH_WORK/stats.jsonl" > "$RAW/propagation-run$r.containers.json"
      log_run propagation "$r" ok "$started" "$load" "$swap"
    else
      stop_stats
      mv "$RAW/propagation-run$r.json" "$RAW/failed/" 2>/dev/null || true
      cp "$BENCH_WORK/stats.jsonl" "$RAW/failed/propagation-run$r.stats.jsonl" || true
      compose logs server > "$RAW/failed/propagation-run$r.server.log" 2>&1 || true
      log_run propagation "$r" failed "$started" "$load" "$swap"
    fi
  done
  compose --profile tools down -v --remove-orphans
}

run_environment() { python3 "$HERE/scripts/environment.py" "$HERE/compose.yml" > "$OUT/environment.json"; }

run_summarize() {
  python3 "$HERE/scripts/summarize.py" "$OUT" > "$OUT/summary.json"
  python3 "$HERE/scripts/render_svg.py" "$OUT/summary.json" "$ROOT/assets/benchmarks"
}

case "${1:-}" in
  eval) run_eval ;;
  server) run_server ;;
  propagation) run_propagation ;;
  environment) run_environment ;;
  summarize) run_summarize ;;
  all) run_environment; run_eval; run_server; run_propagation; run_summarize ;;
  *) sed -n '2,24p' "$0"; exit 2 ;;
esac
