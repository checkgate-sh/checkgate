#!/usr/bin/env bash
# Checkgate v0.2.0 release benchmarks on a dedicated Linux host: a baseline
# release against a candidate commit, built and measured identically.
#
# Run `scripts/release-benchmarks/v0.2.0-linux.sh --help` for usage, and see
# scripts/release-benchmarks/README.md for the method.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LIB="$SCRIPT_DIR/lib"
REPO="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null || true)"

# Published release commits: a tag that no longer points here was moved.
KNOWN_TAG_v0_1_31="33b9d5f947a8a1302196c89ba863f1540d981c16"
# Same images (and digests) as the CI service containers.
PG_IMAGE="postgres:16@sha256:ca0bd484cb98bf4b24eb1010e73fb3fcbd6714d240fbc1a10eea5b7dbecb641d"
REDIS_IMAGE="redis:7@sha256:4fa24486b8bcca8eec45ee0eb166edc674795e53a2b53d1a9ef263eecebaac85"

BASELINE="v0.1.31"
CANDIDATE=""
OUTPUT=""
MODE="run"
ROUNDS=3
SUITES="core,node,snapshot,sse"
THRESHOLD="0.10"
QUICK=0
STRICT=0
KEEP_WORK=0
WORK=""
PG_STORAGE="tmpfs"
SSE_CLIENTS="1,100,500,1000"
SSE_UPDATES=200
CONCURRENCY="1,10,50,100"
SNAP_WARMUP=10
SNAP_SECS=30
CORE_ARGS=""
NODE_ARGS=""
BUSY_LIMIT="0.10"
SERVER_CPUS=""
LOAD_CPUS=""
DATA_CPUS=""
PG_PORT=55491
REDIS_PORT=56391
SERVER_PORT=39101

usage() {
  cat <<'EOF'
Usage: scripts/release-benchmarks/v0.2.0-linux.sh [options]

Builds a baseline and a candidate Checkgate release in isolated worktrees and
compares them on this host: the Rust evaluation core, Node SDK local
evaluation, the /flags/snapshot endpoint and SSE update propagation, with
resource usage. Runs alternate Baseline → Candidate → Candidate → Baseline
per round. Writes summary.md, summary.json (PASS / FAIL / INCONCLUSIVE per
metric, suite and overall), raw measurements, logs and environment metadata.

Options:
  --baseline REF     Baseline tag or commit (default v0.1.31). Resolved to a
                     commit SHA; branch names are refused (they move).
  --candidate REF    Candidate commit SHA or tag (required). Same rules.
  --output DIR       Results directory (default ./benchmark-results/v0.2.0-<UTC time>).
                     Must not exist yet, or be empty.
  --check            Only validate prerequisites and the environment: no
                     builds, containers, worktrees or files. Exit 0 when the
                     host can run the benchmark (warnings allowed).
  --rounds N         Rounds of B→C→C→B per suite (default 3: 6 runs per side).
  --suites LIST      Comma-separated: core,node,snapshot,sse (default all).
  --quick            Short smoke run (1 round, short windows, 1 and 100 SSE
                     clients). Its verdicts are INCONCLUSIVE by design: it
                     checks that the workflow works, not performance.
  --threshold X      Regression threshold as a fraction (default 0.10).
  --sse-clients LIST SSE client counts (default 1,100,500,1000).
  --concurrency LIST Snapshot concurrency levels (default 1,10,50,100).
  --server-cpus SET  CPUs for the server process (taskset list, e.g. 2-3).
  --load-cpus SET    CPUs for load generators and SDK clients.
  --data-cpus SET    CPUs for the PostgreSQL and Redis containers.
                     Default split: data 0-1, server 2-3, load 4-(N-1) on 8+
                     CPUs; data 0, server 1, load 2-(N-1) on 4-7 CPUs.
  --pg-storage MODE  tmpfs (default: no disk noise) or disk.
  --work-dir DIR     Worktrees and build directories (default <output>/work).
  --keep-work        Keep the worktrees, builds and containers afterwards.
  --strict           Treat environment warnings as failures.
  --analyze-only DIR Recompute summary.md/summary.json from DIR/raw (e.g.
                     with another --threshold). Runs nothing.
  -h, --help         Show this help.

Needs: Linux, Docker (native daemon), git, rustup with the toolchain both
refs pin (rust-toolchain.toml), Node.js 22+, npm, Python 3.11+, taskset
(util-linux), curl. Nothing is installed or reconfigured on the host;
`npm ci` and `cargo` fetch packages into the temporary worktrees and
caches, and the two pinned container images are pulled.

Cleanup removes only what this run created: its two worktrees, its build
directories and its two containers (named cgrb-<run id>-pg/-redis). Other
containers, volumes, worktrees and data are never touched.

Example:
  scripts/release-benchmarks/v0.2.0-linux.sh --check --candidate 0123abcd...
  scripts/release-benchmarks/v0.2.0-linux.sh \
    --baseline v0.1.31 --candidate 0123abcd... --output ./benchmark-results
EOF
}

die() { echo "error: $*" >&2; exit 2; }

ANALYZE_DIR=""
while [ $# -gt 0 ]; do
  case "$1" in
    --baseline) BASELINE="${2:?--baseline needs a value}"; shift 2 ;;
    --candidate) CANDIDATE="${2:?--candidate needs a value}"; shift 2 ;;
    --output) OUTPUT="${2:?--output needs a value}"; shift 2 ;;
    --check) MODE="check"; shift ;;
    --rounds) ROUNDS="${2:?}"; shift 2 ;;
    --suites) SUITES="${2:?}"; shift 2 ;;
    --quick) QUICK=1; shift ;;
    --threshold) THRESHOLD="${2:?}"; shift 2 ;;
    --sse-clients) SSE_CLIENTS="${2:?}"; shift 2 ;;
    --concurrency) CONCURRENCY="${2:?}"; shift 2 ;;
    --server-cpus) SERVER_CPUS="${2:?}"; shift 2 ;;
    --load-cpus) LOAD_CPUS="${2:?}"; shift 2 ;;
    --data-cpus) DATA_CPUS="${2:?}"; shift 2 ;;
    --pg-storage) PG_STORAGE="${2:?}"; shift 2 ;;
    --work-dir) WORK="${2:?}"; shift 2 ;;
    --keep-work) KEEP_WORK=1; shift ;;
    --strict) STRICT=1; shift ;;
    --analyze-only) MODE="analyze"; ANALYZE_DIR="${2:?}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "unknown option: $1" ;;
  esac
done

if [ "$MODE" = analyze ]; then
  exec python3 "$LIB/analyze.py" "$ANALYZE_DIR" --threshold "$THRESHOLD"
fi

[[ "$ROUNDS" =~ ^[1-9][0-9]*$ ]] || die "--rounds must be a positive integer"
[[ "$PG_STORAGE" =~ ^(tmpfs|disk)$ ]] || die "--pg-storage must be tmpfs or disk"
for s in ${SUITES//,/ }; do [[ "$s" =~ ^(core|node|snapshot|sse)$ ]] || die "unknown suite: $s"; done
if [ "$QUICK" = 1 ]; then
  ROUNDS=1; SSE_CLIENTS="1,100"; SSE_UPDATES=30; CONCURRENCY="1,10"; SNAP_WARMUP=2; SNAP_SECS=5
  CORE_ARGS="--samples 15 --batch 5000 --warmup 20000"; NODE_ARGS="--samples 20 --batch 5000 --warmup 20000"
fi
has_suite() { [[ ",$SUITES," == *",$1,"* ]]; }

# ── Checks ──────────────────────────────────────────────────────────────────

FAILS=0; WARNS=0
CHECK_LOG=()
ok()   { CHECK_LOG+=("OK    $*"); echo "  ok    $*"; }
warn() { CHECK_LOG+=("WARN  $*"); echo "  WARN  $*"; WARNS=$((WARNS + 1)); }
fail() { CHECK_LOG+=("FAIL  $*"); echo "  FAIL  $*"; FAILS=$((FAILS + 1)); }

# Busy share of all CPUs over $1 seconds, from /proc/stat (0..1).
host_busy() {
  python3 - "$1" <<'PY'
import sys, time
def snap():
    v = [int(x) for x in open("/proc/stat").readline().split()[1:]]
    return sum(v), v[3] + v[4]
a = snap(); time.sleep(float(sys.argv[1])); b = snap()
t = b[0] - a[0]
print(f"{(1 - (b[1] - a[1]) / t) if t else 0:.4f}")
PY
}

resolve_ref() { # name ref out-var: sets out-var to the commit SHA, or records a failure
  local name=$1 ref=$2 var=$3 sha
  if [ -z "$ref" ]; then fail "$name: no ref given (--$name)"; return 1; fi
  if ! sha=$(git -C "$REPO" rev-parse -q --verify "$ref^{commit}" 2>/dev/null); then
    fail "$name: '$ref' is not a commit in $REPO (fetch it first: git fetch --tags origin)"; return 1
  fi
  if [[ ! "$ref" =~ ^[0-9a-f]{7,40}$ ]] && ! git -C "$REPO" show-ref -q --verify "refs/tags/$ref"; then
    fail "$name: '$ref' is a branch or other moving reference; pass a tag or commit SHA ($sha)"; return 1
  fi
  printf -v "$var" '%s' "$sha"
  ok "$name $ref → $sha"
}

run_checks() {
  echo "Environment checks"
  [ "$(uname -s)" = Linux ] && ok "Linux $(uname -r)" || fail "not Linux ($(uname -s)); this workflow reads /proc"
  [ -n "$REPO" ] && ok "repository $REPO" || fail "run the script from a Checkgate git checkout"
  [ "${BASH_VERSINFO[0]}" -ge 4 ] && ok "bash $BASH_VERSION" || fail "bash 4+ needed"
  local c missing=()
  for c in git docker cargo rustup node npm python3 taskset curl; do
    command -v "$c" >/dev/null 2>&1 || missing+=("$c")
  done
  [ ${#missing[@]} -eq 0 ] && ok "commands present" || fail "missing commands: ${missing[*]} (install them; this script installs nothing)"

  # Docker
  if command -v docker >/dev/null 2>&1; then
    local dv ctx
    if dv=$(docker info --format '{{.ServerVersion}}' 2>/dev/null); then
      ok "Docker daemon $dv"
      ctx=$(docker context show 2>/dev/null || echo default)
      if [ "$ctx" != default ] || docker info --format '{{.OperatingSystem}}' 2>/dev/null | grep -qi 'desktop'; then
        warn "Docker context '$ctx' looks like Docker Desktop (a VM): containers do not run on this host's CPUs. Use the native daemon (DOCKER_CONTEXT=default)."
      fi
      local others
      others=$(docker ps --format '{{.Names}} ({{.Image}})' 2>/dev/null | grep -v '^cgrb-' || true)
      if [ -n "$others" ]; then
        warn "other running containers compete for CPU: $(echo "$others" | tr '\n' ' ')"
      else
        ok "no other running containers"
      fi
    else
      fail "Docker daemon not reachable (docker info failed)"
    fi
  fi

  # CPU / memory / disk
  local n; n=$(nproc)
  if [ "$n" -lt 4 ]; then fail "$n CPUs; at least 4 needed (8+ recommended)"
  elif [ "$n" -lt 8 ]; then warn "$n CPUs; 8+ recommended so server, load and data stores get their own cores"
  else ok "$n CPUs"; fi
  local mem_kib avail_kib
  mem_kib=$(awk '/^MemTotal:/{print $2}' /proc/meminfo); avail_kib=$(awk '/^MemAvailable:/{print $2}' /proc/meminfo)
  if [ "$mem_kib" -lt $((8 * 1024 * 1024)) ]; then fail "memory $((mem_kib / 1024 / 1024)) GiB; 8 GiB needed (16 recommended)"
  elif [ "$mem_kib" -lt $((15 * 1024 * 1024)) ]; then warn "memory $((mem_kib / 1024 / 1024)) GiB; 16 GiB recommended"
  else ok "memory $((mem_kib / 1024 / 1024)) GiB"; fi
  [ "$avail_kib" -ge $((6 * 1024 * 1024)) ] && ok "available memory $((avail_kib / 1024 / 1024)) GiB" || warn "only $((avail_kib / 1024 / 1024)) GiB available"
  local probe="${WORK:-${OUTPUT:-$PWD}}" free_kib
  while [ ! -d "$probe" ]; do probe=$(dirname "$probe"); done
  free_kib=$(df -Pk "$probe" | awk 'NR==2{print $4}')
  [ "$free_kib" -ge $((20 * 1024 * 1024)) ] && ok "disk $((free_kib / 1024 / 1024)) GiB free at $probe" || fail "disk: $((free_kib / 1024 / 1024)) GiB free at $probe; 20 GiB needed (two release builds)"
  if has_suite sse; then
    local hard; hard=$(ulimit -Hn)
    if [ "$hard" != unlimited ] && [ "$hard" -lt 8192 ]; then
      fail "open-file hard limit $hard; 8192+ needed for 1,000 SSE clients (raise it in limits.conf; not changed here)"
    else ok "open-file hard limit $hard"; fi
  fi

  # Toolchains
  if command -v node >/dev/null 2>&1; then
    local nv; nv=$(node -p 'process.versions.node' 2>/dev/null)
    [ "${nv%%.*}" -ge 22 ] 2>/dev/null && ok "Node.js $nv" || fail "Node.js $nv; 22+ needed"
  fi
  if command -v python3 >/dev/null 2>&1; then
    python3 -c 'import sys; sys.exit(sys.version_info < (3, 11))' && ok "Python $(python3 -V 2>&1 | cut -d' ' -f2)" \
      || warn "Python $(python3 -V 2>&1 | cut -d' ' -f2); 3.11+ is what the 0.2.0 workspace expects"
  fi

  # Revisions
  if [ -n "$REPO" ]; then
    resolve_ref baseline "$BASELINE" BASE_SHA
    resolve_ref candidate "$CANDIDATE" CAND_SHA
    if [ "$BASELINE" = v0.1.31 ] && [ -n "${BASE_SHA:-}" ] && [ "$BASE_SHA" != "$KNOWN_TAG_v0_1_31" ]; then
      warn "tag v0.1.31 points to $BASE_SHA, not the published release $KNOWN_TAG_v0_1_31"
    fi
    if [ -n "${BASE_SHA:-}" ] && [ "${BASE_SHA:-}" = "${CAND_SHA:-}" ]; then warn "baseline and candidate are the same commit"; fi
    local tc
    for side in BASE CAND; do
      local sha_var="${side}_SHA"; local sha="${!sha_var:-}"
      [ -n "$sha" ] || continue
      tc=$(git -C "$REPO" show "$sha:rust-toolchain.toml" 2>/dev/null | sed -n 's/^channel *= *"\(.*\)"/\1/p')
      [ -n "$tc" ] || { fail "$side: no rust-toolchain.toml channel at $sha"; continue; }
      eval "${side}_TOOLCHAIN=\$tc"
      if command -v rustup >/dev/null 2>&1 && rustup toolchain list 2>/dev/null | grep -q "^$tc"; then
        ok "$side pins Rust $tc (installed)"
      else
        fail "$side pins Rust $tc, not installed: rustup toolchain install $tc --profile minimal (not run here)"
      fi
    done
    if [ -n "${BASE_TOOLCHAIN:-}" ] && [ "${BASE_TOOLCHAIN:-}" != "${CAND_TOOLCHAIN:-}" ]; then
      warn "toolchains differ (baseline ${BASE_TOOLCHAIN}, candidate ${CAND_TOOLCHAIN}); each ref is built with its own pin"
    fi
  fi

  # Ports
  local p
  for p in "$PG_PORT" "$REDIS_PORT" "$SERVER_PORT"; do
    if (exec 3<>"/dev/tcp/127.0.0.1/$p") 2>/dev/null; then fail "port $p is in use"; else ok "port $p free"; fi
  done

  # Competing work and host settings (recorded; changed by nobody but you)
  local busy load1
  busy=$(host_busy 3); load1=$(cut -d' ' -f1 /proc/loadavg)
  if python3 -c "import sys; sys.exit(not ($busy > $BUSY_LIMIT))"; then
    warn "host is $(python3 -c "print(f'{$busy:.0%}')") busy while idle should be ~0 (load1 $load1): stop other work first"
  else ok "host idle ($(python3 -c "print(f'{$busy:.1%}')") busy over 3 s, load1 $load1)"; fi
  local other
  other=$(pgrep -af '/target/(release|debug)/server( |$)' 2>/dev/null | head -3 || true)
  [ -z "$other" ] && ok "no other Checkgate processes" || warn "other Checkgate processes running: $(echo "$other" | tr '\n' ';')"
  local swap; swap=$(awk '/^SwapTotal/{t=$2} /^SwapFree/{f=$2} END{print int((t-f)/1024)}' /proc/meminfo)
  [ "$swap" -le 64 ] && ok "swap in use ${swap} MiB" || warn "swap in use ${swap} MiB: memory pressure distorts latency"
  local gov=""; [ -r /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor ] && gov=$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor)
  if [ -z "$gov" ]; then ok "CPU governor: not exposed"
  elif [ "$gov" = performance ]; then ok "CPU governor performance"
  else warn "CPU governor '$gov': frequency scaling adds variance (e.g. sudo cpupower frequency-set -g performance; not changed here)"; fi
  local virt="none"; command -v systemd-detect-virt >/dev/null 2>&1 && virt=$(systemd-detect-virt 2>/dev/null || echo none)
  [ "$virt" = none ] && ok "bare metal" || warn "virtualized ($virt): neighbours and steal time can distort results"
  local ps
  for ps in /sys/class/power_supply/*/online; do
    [ -r "$ps" ] && [ "$(cat "$ps")" = 0 ] && warn "running on battery ($ps)"
  done
  echo "Checks: $FAILS failed, $WARNS warnings"
}

# ── Run-time helpers ────────────────────────────────────────────────────────

CREATED_WORKTREES=(); CREATED_CONTAINERS=(); BG_PIDS=()
cleanup() {
  local pid
  for pid in "${BG_PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
  if [ "$KEEP_WORK" = 1 ]; then
    echo "Kept: ${CREATED_WORKTREES[*]} ${CREATED_CONTAINERS[*]}"; return
  fi
  local c w
  for c in "${CREATED_CONTAINERS[@]}"; do docker rm -f "$c" >/dev/null 2>&1 || true; done
  for w in "${CREATED_WORKTREES[@]}"; do
    case "$w" in "$WORK"/*) git -C "$REPO" worktree remove --force "$w" >/dev/null 2>&1 || true ;; esac
  done
  # Build directories this run created (never anything outside $WORK).
  [ -n "$WORK" ] && [ -d "$WORK" ] && rm -rf "$WORK/target-baseline" "$WORK/target-candidate" "$WORK/tmp"
}

log() { echo "[$(date -u +%H:%M:%S)] $*" | tee -a "$OUTPUT/logs/driver.log"; }

cpu_split() {
  local n; n=$(nproc)
  if [ -z "$SERVER_CPUS$LOAD_CPUS$DATA_CPUS" ]; then
    if [ "$n" -ge 8 ]; then DATA_CPUS="0-1"; SERVER_CPUS="2-3"; LOAD_CPUS="4-$((n - 1))"
    else DATA_CPUS="0"; SERVER_CPUS="1"; LOAD_CPUS="2-$((n - 1))"; fi
  fi
  [ -n "$SERVER_CPUS" ] && [ -n "$LOAD_CPUS" ] && [ -n "$DATA_CPUS" ] || die "--server-cpus, --load-cpus and --data-cpus go together"
  BENCH_CPU="${SERVER_CPUS%%[-,]*}"
}

write_environment() {
  python3 - "$OUTPUT/environment.json" <<PY
import json, os, platform, subprocess, sys
def sh(c):
    try: return subprocess.run(c, shell=True, capture_output=True, text=True, timeout=20).stdout.strip()
    except Exception as e: return f"error: {e}"
cpu_model = ""
for line in open("/proc/cpuinfo"):
    if line.startswith("model name"): cpu_model = line.split(":", 1)[1].strip(); break
mem = int(open("/proc/meminfo").readline().split()[1])
gov_path = "/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"
doc = {
  "schema": "checkgate.release-benchmark-env/1",
  "run_id": "$RUN_ID",
  "started_utc": "$STARTED",
  "baseline": {"ref": "$BASELINE", "sha": "$BASE_SHA", "rust_toolchain": "${BASE_TOOLCHAIN:-}"},
  "candidate": {"ref": "$CANDIDATE", "sha": "$CAND_SHA", "rust_toolchain": "${CAND_TOOLCHAIN:-}"},
  "driver": {"repo": "$REPO", "head": sh("git -C '$REPO' rev-parse HEAD"), "dirty": bool(sh("git -C '$REPO' status --porcelain -- scripts/release-benchmarks")), "script": "scripts/release-benchmarks/v0.2.0-linux.sh"},
  "settings": {"rounds": $ROUNDS, "order": "B,C,C,B per round", "suites": "$SUITES", "quick": bool($QUICK),
               "threshold": $THRESHOLD, "sse_clients": "$SSE_CLIENTS", "sse_updates": $SSE_UPDATES,
               "snapshot_concurrency": "$CONCURRENCY", "snapshot_warmup_s": $SNAP_WARMUP, "snapshot_window_s": $SNAP_SECS,
               "pg_storage": "$PG_STORAGE", "pg_image": "$PG_IMAGE", "redis_image": "$REDIS_IMAGE",
               "busy_limit_before_run": $BUSY_LIMIT,
               "harness_tools": "throughput example, bench/public/sdk/*.mjs and the Node SDK used as SSE client come from the candidate worktree for both sides; core_ab.rs and eval.mjs are identical files run against each side's build"},
  "host": {"kernel": platform.release(), "cpu_model": cpu_model, "cpus": os.cpu_count(), "mem_gib": round(mem / 1024 / 1024, 1),
           "governor": open(gov_path).read().strip() if os.path.exists(gov_path) else None,
           "virtualization": sh("systemd-detect-virt 2>/dev/null || echo unknown"),
           "server_cpuset": "$SERVER_CPUS", "load_cpuset": "$LOAD_CPUS", "data_cpuset": "$DATA_CPUS", "bench_cpu": "$BENCH_CPU",
           "lscpu": sh("lscpu 2>/dev/null"), "os_release": sh("cat /etc/os-release 2>/dev/null"),
           "smt": sh("cat /sys/devices/system/cpu/smt/active 2>/dev/null"),
           "no_turbo": sh("cat /sys/devices/system/cpu/intel_pstate/no_turbo 2>/dev/null"),
           "thp": sh("cat /sys/kernel/mm/transparent_hugepage/enabled 2>/dev/null"),
           "nofile_soft": sh("ulimit -n"), "nofile_hard": sh("ulimit -Hn")},
  "tools": {"docker": sh("docker version --format '{{.Server.Version}}'"), "node": sh("node -v"), "npm": sh("npm -v"),
            "python": platform.python_version(), "rustc": sh("rustc -V"), "rustup": sh("rustup -V 2>/dev/null | head -1"),
            "git": sh("git --version")},
  "checks": $(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${CHECK_LOG[@]}"),
}
json.dump(doc, open(sys.argv[1], "w"), indent=2)
PY
}

prepare_worktree() { # side sha
  local side=$1 sha=$2 wt="$WORK/$1"
  [ -e "$wt" ] && die "$wt exists; choose another --work-dir"
  git -C "$REPO" worktree add --detach "$wt" "$sha" >>"$OUTPUT/logs/prepare-$side.log" 2>&1 || die "git worktree add $wt failed (logs/prepare-$side.log)"
  CREATED_WORKTREES+=("$wt")
  # The identical core harness, registered in this temporary worktree only.
  cp "$LIB/core_ab.rs" "$wt/bench/benches/release_core_ab.rs"
  printf '\n[[bench]]\nname = "release_core_ab"\npath = "benches/release_core_ab.rs"\nharness = false\n' >> "$wt/bench/Cargo.toml"
}

build_side() { # side
  local side=$1 wt="$WORK/$1" target="$WORK/target-$1" l="$OUTPUT/logs/build-$1.log"
  log "build $side ($(git -C "$wt" rev-parse --short HEAD)) into $target"
  (
    cd "$wt" || exit 1
    export CARGO_TARGET_DIR="$target"
    cargo build --release --locked -p server --bin server || exit 1
    if [ "$side" = candidate ]; then cargo build --release --locked -p server --example throughput || exit 1; fi
    cargo bench --locked -p checkgate-bench --bench release_core_ab --no-run --message-format=json \
      | python3 -c '
import json, sys
for line in sys.stdin:
    try: m = json.loads(line)
    except ValueError: continue
    if m.get("reason") == "compiler-artifact" and m.get("target", {}).get("name") == "release_core_ab" and m.get("executable"):
        print(m["executable"])' > "$target/core_ab.path" || exit 1
    [ -s "$target/core_ab.path" ] || exit 1
    if has_suite node || has_suite sse; then
      (cd sdks/nodejs && npm ci --no-audit --no-fund && npm run build) || exit 1
    fi
  ) >>"$l" 2>&1 || die "build of $side failed — see $l"
  eval "CORE_BIN_$side=\$(cat \"$target/core_ab.path\")"
  eval "SERVER_BIN_$side=\"$target/release/server\""
  [ -x "$target/release/server" ] || die "no server binary for $side"
}

start_datastores() {
  local pgopt=()
  [ "$PG_STORAGE" = tmpfs ] && pgopt=(--tmpfs /var/lib/postgresql/data:rw,size=4g)
  log "pull pinned images"
  docker pull -q "$PG_IMAGE" >>"$OUTPUT/logs/prepare.log" 2>&1 || die "docker pull $PG_IMAGE failed"
  docker pull -q "$REDIS_IMAGE" >>"$OUTPUT/logs/prepare.log" 2>&1 || die "docker pull $REDIS_IMAGE failed"
  PG_NAME="cgrb-$RUN_ID-pg"; REDIS_NAME="cgrb-$RUN_ID-redis"
  docker run -d --name "$PG_NAME" --label "checkgate.release-bench.run=$RUN_ID" \
    --cpuset-cpus "$DATA_CPUS" --memory 4g -p "127.0.0.1:$PG_PORT:5432" "${pgopt[@]}" \
    -e POSTGRES_USER=postgres -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=postgres \
    "$PG_IMAGE" -c max_connections=200 >>"$OUTPUT/logs/prepare.log" 2>&1 || die "start PostgreSQL failed"
  CREATED_CONTAINERS+=("$PG_NAME")
  docker run -d --name "$REDIS_NAME" --label "checkgate.release-bench.run=$RUN_ID" \
    --cpuset-cpus "$DATA_CPUS" --memory 1g -p "127.0.0.1:$REDIS_PORT:6379" \
    "$REDIS_IMAGE" redis-server --save '' --appendonly no >>"$OUTPUT/logs/prepare.log" 2>&1 || die "start Redis failed"
  CREATED_CONTAINERS+=("$REDIS_NAME")
  local i
  for i in $(seq 1 60); do
    docker exec "$PG_NAME" pg_isready -U postgres >/dev/null 2>&1 && docker exec "$REDIS_NAME" redis-cli ping >/dev/null 2>&1 && return 0
    sleep 1
  done
  die "data stores did not become ready"
}

fresh_db() { # name
  docker exec "$PG_NAME" psql -U postgres -qc "DROP DATABASE IF EXISTS $1 WITH (FORCE)" -c "CREATE DATABASE $1" >/dev/null 2>&1 \
    && docker exec "$REDIS_NAME" redis-cli FLUSHALL >/dev/null 2>&1
}

start_server() { # side db logfile -> sets SERVER_PID
  local bin_var="SERVER_BIN_$1"; local pub="$WORK/tmp/public-$RANDOM"
  mkdir -p "$pub"
  DATABASE_URL="postgres://postgres:postgres@127.0.0.1:$PG_PORT/$2" REDIS_URL="redis://127.0.0.1:$REDIS_PORT" \
    PORT="$SERVER_PORT" SESSION_SECRET=bench-session-secret-0123456789-0123456789-0123 \
    SETUP_TOKEN=bench-setup-token-0123456789 COOKIE_SECURE=false RUST_LOG=warn PUBLIC_DIR="$pub" \
    RATE_LIMIT_AUTH_PER_MINUTE=0 RATE_LIMIT_API_PER_MINUTE=0 RATE_LIMIT_SDK_PER_MINUTE=0 \
    RATE_LIMIT_SSE_PER_MINUTE=0 RATE_LIMIT_INGEST_PER_MINUTE=0 \
    taskset -c "$SERVER_CPUS" "${!bin_var}" >"$3" 2>&1 &
  SERVER_PID=$!
  BG_PIDS+=("$SERVER_PID")
  local i
  for i in $(seq 1 120); do
    kill -0 "$SERVER_PID" 2>/dev/null || return 1
    curl -sf "http://127.0.0.1:$SERVER_PORT/health/ready" >/dev/null 2>&1 && return 0
    sleep 0.5
  done
  return 1
}

stop_server() { # -> 0 if it was still running when asked to stop
  local alive=0
  kill -0 "$SERVER_PID" 2>/dev/null || alive=1
  kill -TERM "$SERVER_PID" 2>/dev/null
  local i
  for i in $(seq 1 60); do kill -0 "$SERVER_PID" 2>/dev/null || break; sleep 0.5; done
  kill -KILL "$SERVER_PID" 2>/dev/null || true
  wait "$SERVER_PID" 2>/dev/null || true
  return $alive
}

wait_idle() { # -> prints busy share before the run
  local busy i
  for i in $(seq 1 30); do
    busy=$(host_busy 3)
    python3 -c "import sys; sys.exit(not ($busy <= $BUSY_LIMIT))" && { echo "$busy"; return 0; }
  done
  echo "$busy"; return 1
}

SEQ=0
run_one() { # suite side round
  local suite=$1 side=$2 round=$3
  SEQ=$((SEQ + 1))
  local tag="$suite-$side-r$round-$SEQ" rdir="$OUTPUT/raw/$suite" l="$OUTPUT/logs/$suite/$side-r$round-$SEQ.log"
  mkdir -p "$rdir" "$OUTPUT/logs/$suite" "$WORK/tmp"
  local reasons=() valid=true server_exited=false status=0 busy load1 started
  local result="raw/$suite/$side-r$round-$SEQ.json" sampler="raw/$suite/$side-r$round-$SEQ.samples.jsonl" audit=""
  busy=$(wait_idle) || reasons+=("host ${busy} busy before the run (limit $BUSY_LIMIT)")
  load1=$(cut -d' ' -f1 /proc/loadavg); started=$(date -u +%FT%TZ)
  log "run $SEQ: $suite $side round $round (host busy $busy, load1 $load1)"
  local stop="$WORK/tmp/stop-$SEQ" cand_wt="$WORK/candidate" wt="$WORK/$side"
  rm -f "$stop"
  case "$suite" in
    core|node)
      local bin_var="CORE_BIN_$side"
      if [ "$suite" = core ]; then
        # shellcheck disable=SC2086
        taskset -c "$BENCH_CPU" "${!bin_var}" $CORE_ARGS >"$OUTPUT/$result" 2>"$l" &
      else
        # shellcheck disable=SC2086
        CHECKGATE_SDK="$wt/sdks/nodejs/index.js" taskset -c "$BENCH_CPU" \
          node "$cand_wt/bench/public/sdk/eval.mjs" $NODE_ARGS --out "$OUTPUT/$result" >"$l" 2>&1 &
      fi
      local hp=$!
      python3 "$LIB/sampler.py" --pid "$hp" --port "$SERVER_PORT" --stop-file "$stop" --out "$OUTPUT/$sampler" &
      local sp=$!
      wait "$hp"; status=$?
      touch "$stop"; wait "$sp" 2>/dev/null
      ;;
    snapshot|sse)
      local db="cgrb_${suite}_${SEQ}"
      if ! fresh_db "$db"; then status=97; reasons+=("could not reset the database")
      elif ! start_server "$side" "$db" "$OUTPUT/logs/$suite/$side-r$round-$SEQ.server.log"; then
        status=98; reasons+=("server did not become ready"); server_exited=true; stop_server || true
      else
        python3 "$LIB/sampler.py" --pid "$SERVER_PID" --port "$SERVER_PORT" --stop-file "$stop" --out "$OUTPUT/$sampler" &
        local sp=$!
        local session="$WORK/tmp/session-$SEQ.json"
        if ! taskset -c "$LOAD_CPUS" node "$cand_wt/bench/public/sdk/seed.mjs" --url "http://127.0.0.1:$SERVER_PORT" \
            --setup-token bench-setup-token-0123456789 --out "$session" >"$l" 2>&1; then
          status=96; reasons+=("seeding failed")
        elif [ "$suite" = snapshot ]; then
          taskset -c "$LOAD_CPUS" "$WORK/target-candidate/release/examples/throughput" \
            --url "http://127.0.0.1:$SERVER_PORT" --session "$session" --concurrency "$CONCURRENCY" \
            --warmup-secs "$SNAP_WARMUP" --secs "$SNAP_SECS" --server-pid "$SERVER_PID" \
            --out "$OUTPUT/$result" >>"$l" 2>&1
          status=$?
        else
          audit="raw/$suite/$side-r$round-$SEQ.audit.json"
          local key; key=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['sdk_key'])" "$session")
          taskset -c "$LOAD_CPUS" node "$LIB/sse_audit.mjs" --url "http://127.0.0.1:$SERVER_PORT" --sdk-key "$key" \
            --streams 25 --stop-file "$stop" --out "$OUTPUT/$audit" >>"$l.audit" 2>&1 &
          local ap=$!
          sleep 2
          CHECKGATE_SDK="$cand_wt/sdks/nodejs/index.js" taskset -c "$LOAD_CPUS" \
            node "$cand_wt/bench/public/sdk/propagation.mjs" --session "$session" --clients "$SSE_CLIENTS" \
            --updates "$SSE_UPDATES" --server-pid "$SERVER_PID" --run-tag "r$SEQ" --out "$OUTPUT/$result" >>"$l" 2>&1
          status=$?
          touch "$stop"; wait "$ap" 2>/dev/null
        fi
        touch "$stop"; wait "$sp" 2>/dev/null
        if ! stop_server; then server_exited=true; reasons+=("server exited during the run"); fi
      fi
      ;;
  esac
  [ "$status" -eq 0 ] || reasons+=("harness exit status $status")
  if [ -f "$OUTPUT/$sampler" ]; then
    local s
    s=$(python3 - "$OUTPUT/$sampler" <<'PY'
import json, sys
summ = None
for line in open(sys.argv[1]):
    row = json.loads(line)
    if "summary" in row: summ = row["summary"]
if summ:
    out = []
    if (summ.get("host_steal_max") or 0) > 0.05: out.append(f"host CPU steal {summ['host_steal_max']:.1%}")
    if (summ.get("swap_growth_mib") or 0) > 64: out.append(f"swap grew {summ['swap_growth_mib']} MiB")
    print("; ".join(out))
PY
)
    [ -n "$s" ] && reasons+=("$s")
  fi
  [ ${#reasons[@]} -eq 0 ] || valid=false
  if [ "$valid" = false ]; then
    mkdir -p "$OUTPUT/failed"
    cp "$l" "$OUTPUT/failed/$tag.log" 2>/dev/null || true
    cp "$OUTPUT/logs/$suite/$side-r$round-$SEQ.server.log" "$OUTPUT/failed/$tag.server.log" 2>/dev/null || true
    log "  INVALID: ${reasons[*]}"
  fi
  python3 - "$OUTPUT/raw/runs.jsonl" <<PY
import json, sys
rec = {"suite": "$suite", "side": "$side", "round": $round, "seq": $SEQ, "started": "$started",
       "ended": "$(date -u +%FT%TZ)", "status": $status, "valid": "$valid" == "true",
       "invalid_reasons": $(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${reasons[@]}"),
       "server_exited": "$server_exited" == "true", "host_busy_before": ${busy:-None}, "load1_before": ${load1:-None},
       "result": "$result", "sampler": "$sampler", "audit": "$audit" or None,
       "log": "logs/$suite/$side-r$round-$SEQ.log"}
open(sys.argv[1], "a").write(json.dumps(rec) + "\n")
PY
}

# ── Main ────────────────────────────────────────────────────────────────────

BASE_SHA=""; CAND_SHA=""
if [ "$MODE" = check ]; then
  run_checks
  [ "$FAILS" -eq 0 ] || exit 1
  [ "$STRICT" = 1 ] && [ "$WARNS" -gt 0 ] && exit 1
  exit 0
fi

[ -n "$CANDIDATE" ] || { usage >&2; die "--candidate is required"; }
STARTED=$(date -u +%FT%TZ)
RUN_ID=$(date -u +%Y%m%dT%H%M%SZ)
OUTPUT="${OUTPUT:-./benchmark-results/v0.2.0-$RUN_ID}"
if [ -e "$OUTPUT" ] && [ -n "$(ls -A "$OUTPUT" 2>/dev/null)" ]; then die "$OUTPUT is not empty"; fi
mkdir -p "$OUTPUT"/{raw,logs} || die "cannot create $OUTPUT"
OUTPUT="$(cd "$OUTPUT" && pwd)"
WORK="${WORK:-$OUTPUT/work}"; mkdir -p "$WORK/tmp"; WORK="$(cd "$WORK" && pwd)"
run_checks >"$OUTPUT/logs/checks.log" 2>&1
cat "$OUTPUT/logs/checks.log"
if [ "$FAILS" -gt 0 ] || { [ "$STRICT" = 1 ] && [ "$WARNS" -gt 0 ]; }; then
  die "environment checks failed (logs/checks.log); fix them or see --help"
fi
trap cleanup EXIT
trap 'exit 130' INT TERM
export RUSTUP_AUTO_INSTALL=0 CARGO_TERM_COLOR=never PYO3_PYTHON="${PYO3_PYTHON:-$(command -v python3)}"
ulimit -n "$(ulimit -Hn)" 2>/dev/null || true
cpu_split
write_environment
log "run $RUN_ID: baseline $BASELINE ($BASE_SHA) vs candidate $CANDIDATE ($CAND_SHA); output $OUTPUT"
log "CPUs: server $SERVER_CPUS, load $LOAD_CPUS, data stores $DATA_CPUS, microbenchmarks $BENCH_CPU"

prepare_worktree baseline "$BASE_SHA"
prepare_worktree candidate "$CAND_SHA"
build_side baseline
build_side candidate

for suite in core node snapshot sse; do
  has_suite "$suite" || continue
  # Data stores only for the server suites, so they add no background work
  # to the microbenchmarks.
  if [[ "$suite" =~ ^(snapshot|sse)$ ]] && [ -z "${PG_NAME:-}" ]; then start_datastores; fi
  log "suite $suite: warm-up (one unrecorded run per side)"
  # Warm-up runs are written under raw/warmup and never analysed.
  SAVED_OUT="$OUTPUT"
  for side in baseline candidate; do
    OUTPUT="$SAVED_OUT/warmup"; mkdir -p "$OUTPUT/raw" "$OUTPUT/logs"
    run_one "$suite" "$side" 0 >/dev/null 2>&1 || true
    OUTPUT="$SAVED_OUT"
  done
  for r in $(seq 1 "$ROUNDS"); do
    for side in baseline candidate candidate baseline; do
      run_one "$suite" "$side" "$r"
    done
  done
done

log "analysis"
python3 "$LIB/analyze.py" "$OUTPUT" --threshold "$THRESHOLD" 2>&1 | tee -a "$OUTPUT/logs/driver.log"
if [ "${PIPESTATUS[0]}" -ne 0 ]; then
  log "analysis FAILED; raw data kept — rerun with --analyze-only $OUTPUT"
  exit 1
fi
log "done: $OUTPUT/summary.md"
