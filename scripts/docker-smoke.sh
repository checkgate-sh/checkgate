#!/usr/bin/env bash
# Smoke-tests both deployment modes of a built Checkgate image.
#
#   scripts/docker-smoke.sh [image]      (default: checkgate:latest)
#
# all-in-one: first boot, generated setup token (file only, never logged),
#   setup + login, data surviving a container restart and a container
#   re-create on the same volume.
# external:   connects to a separate PostgreSQL and Redis, starts neither
#   bundled service, writes to the external database, survives a restart.
# misconfiguration: every half-specified or conflicting configuration stops
#   the container with a clear error instead of using the bundled services.
#
# Needs docker, curl and python3. Everything it creates is removed on exit.
set -euo pipefail

IMAGE="${1:-checkgate:latest}"
RUN_ID="cgsmoke-$$"
NET="$RUN_ID-net"
SECRET="smoke-session-secret-0123456789-0123456789-0123456789"
CONTAINERS=()
VOLUMES=()

log() { echo "==> $*"; }
fail() {
  echo "FAIL: $*" >&2
  exit 1
}

cleanup() {
  for c in "${CONTAINERS[@]}"; do docker rm -f "$c" >/dev/null 2>&1 || true; done
  for v in "${VOLUMES[@]}"; do docker volume rm -f "$v" >/dev/null 2>&1 || true; done
  docker network rm "$NET" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
WORK="$(mktemp -d)"
trap cleanup EXIT

docker image inspect "$IMAGE" >/dev/null 2>&1 || fail "image $IMAGE not found — build it first"
docker network create "$NET" >/dev/null

# Host port a container's 3000 is published on.
port_of() { docker port "$1" 3000/tcp | head -1 | sed 's/.*://'; }

wait_healthy() {
  local name="$1" port
  for _ in $(seq 1 120); do
    if [ "$(docker inspect -f '{{.State.Running}}' "$name" 2>/dev/null)" != "true" ]; then
      docker logs "$name" >&2 || true
      fail "$name stopped while starting"
    fi
    port="$(port_of "$name" 2>/dev/null || true)"
    if [ -n "$port" ] && curl -sf "http://127.0.0.1:$port/health" >/dev/null; then
      return 0
    fi
    sleep 1
  done
  docker logs "$name" >&2 || true
  fail "$name did not become healthy"
}

# api NAME METHOD PATH BODY [EXTRA_CURL_ARGS...] — prints the HTTP status and
# leaves the response body in $WORK/body. Pass '' for no body.
api() {
  local name="$1" method="$2" path="$3" body="$4"
  shift 4
  local port data=()
  port="$(port_of "$name")"
  [ -z "$body" ] || data=(--data "$body")
  curl -s -o "$WORK/body" -w '%{http_code}' -X "$method" \
    -H 'Content-Type: application/json' -H 'X-Checkgate-Request: true' \
    -b "$WORK/cookies-$name" -c "$WORK/cookies-$name" \
    "${data[@]}" "$@" "http://127.0.0.1:$port$path"
}

expect() {
  local want="$1" got="$2" what="$3"
  [ "$got" = "$want" ] || fail "$what: expected HTTP $want, got $got ($(cat "$WORK/body" 2>/dev/null))"
}

json_field() { python3 -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]])' "$1" < "$WORK/body"; }

process_names() { docker exec "$1" sh -c 'cat /proc/[0-9]*/comm 2>/dev/null' | sort -u; }

# UIDs of the processes named $2 in container $1 (one per line, deduplicated).
process_uids() {
  docker exec "$1" sh -c '
    for d in /proc/[0-9]*; do
      if [ "$(cat "$d/comm" 2>/dev/null)" = "$0" ]; then
        sed -n "s/^Uid:[[:space:]]*\([0-9]*\).*/\1/p" "$d/status"
      fi
    done' "$2" | sort -u
}

# expect_uid CONTAINER PROCESS USER — every PROCESS runs as USER (and not root).
expect_uid() {
  local want got
  want="$(docker exec "$1" id -u "$3")"
  got="$(process_uids "$1" "$2")"
  [ -n "$got" ] || fail "$1: no $2 process"
  [ "$got" = "$want" ] || fail "$1: $2 runs as uid(s) $(echo "$got" | tr '\n' ' ')— expected $3 ($want)"
}

# Status of a plain GET on the published port (no cookies).
status_of() { curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$(port_of "$1")$2"; }

SETUP='{"workspace_name":"Smoke","project_name":"Smoke","name":"Admin","email":"admin@smoke.test","password":"smoke-password-1"}'
LOGIN='{"email":"admin@smoke.test","password":"smoke-password-1"}'

# ---------------------------------------------------------------------------
log "all-in-one: first boot"
AIO="$RUN_ID-aio"
PGVOL="$RUN_ID-pgdata"
VOLUMES+=("$PGVOL")
CONTAINERS+=("$AIO")
# A fresh tmpfs on /run (as some runtimes and BuildKit builders give) must
# not break the bundled PostgreSQL, whose socket directory lives there.
run_aio() {
  docker run -d --name "$AIO" -p 127.0.0.1::3000 --tmpfs /run \
    -e SESSION_SECRET="$SECRET" -e COOKIE_SECURE=false -e POSTGRES_PASSWORD=smoke-db-password \
    -v "$PGVOL:/var/lib/postgresql/data" "$IMAGE" >/dev/null
}
run_aio
wait_healthy "$AIO"
# Captured before grepping: with pipefail, `docker logs | grep -q` fails when
# grep exits early and docker logs gets SIGPIPE.
LOGS="$(docker logs "$AIO" 2>&1)"
grep -q "Mode: all-in-one" <<<"$LOGS" || fail "all-in-one mode not reported"
PROCS="$(process_names "$AIO")"
for p in postgres redis-server checkgate-serve; do
  grep -qx "$p" <<<"$PROCS" || fail "all-in-one: $p is not running"
done

docker exec "$AIO" test -d /var/lib/redis/appendonlydir || fail "bundled Redis does not write to its /var/lib/redis volume"

log "all-in-one: no service runs as root"
expect_uid "$AIO" checkgate-serve checkgate
expect_uid "$AIO" postgres postgres
expect_uid "$AIO" redis-server redis

log "all-in-one: health endpoints and dashboard headers"
expect 200 "$(status_of "$AIO" /health/live)" "liveness"
expect 200 "$(status_of "$AIO" /health/ready)" "readiness"
[ "$(docker inspect -f '{{.State.Health.Status}}' "$AIO")" != "unhealthy" ] || fail "container HEALTHCHECK reports unhealthy"
HEADERS="$(curl -sI "http://127.0.0.1:$(port_of "$AIO")/")"
grep -qi '^x-content-type-options: nosniff' <<<"$HEADERS" || fail "dashboard index lacks security headers"
grep -qi '^content-security-policy:.*fonts.googleapis.com' <<<"$HEADERS" || fail "dashboard CSP does not allow its web fonts"

TOKEN="$(docker exec "$AIO" cat /var/lib/checkgate/setup-token)"
[ "${#TOKEN}" -eq 64 ] || fail "generated setup token has unexpected length"
[ "$(docker exec "$AIO" stat -c %a /var/lib/checkgate/setup-token)" = "600" ] || fail "token file is not 0600"
LOGS="$(docker logs "$AIO" 2>&1)"
grep -q "setup token" <<<"$LOGS" || fail "log does not point at the token file"
if grep -qF "$TOKEN" <<<"$LOGS"; then fail "setup token appears in the log"; fi

expect 401 "$(api "$AIO" POST /api/setup/complete "$SETUP")" "setup without token"
expect 401 "$(api "$AIO" POST /api/setup/complete "$SETUP" -H 'X-Checkgate-Setup-Token: wrong-token-0123456789')" "setup with wrong token"
expect 200 "$(api "$AIO" GET /api/setup/key '' -H "X-Checkgate-Setup-Token: $TOKEN")" "setup key"
ENV_ID="$(json_field environment_id)"
expect 200 "$(api "$AIO" POST /api/setup/complete "$SETUP" -H "X-Checkgate-Setup-Token: $TOKEN")" "setup"
docker exec "$AIO" test ! -e /var/lib/checkgate/setup-token || fail "token file left after setup"
expect 200 "$(api "$AIO" POST "/api/environments/$ENV_ID/flags" '{"key":"smoke-flag","is_enabled":true,"flag_type":"boolean","rules":[]}')" "create flag"

log "all-in-one: container restart keeps data"
docker restart "$AIO" >/dev/null
wait_healthy "$AIO"
rm -f "$WORK/cookies-$AIO"
expect 200 "$(api "$AIO" POST /api/auth/login "$LOGIN")" "login after restart"
expect 200 "$(api "$AIO" GET "/api/environments/$ENV_ID/flags/smoke-flag" '')" "flag after restart"
expect 404 "$(api "$AIO" POST /api/setup/complete "$SETUP" -H "X-Checkgate-Setup-Token: $TOKEN")" "setup is one-time"

log "all-in-one: re-created container on the same volume keeps data"
docker rm -f "$AIO" >/dev/null
run_aio
wait_healthy "$AIO"
rm -f "$WORK/cookies-$AIO"
expect 200 "$(api "$AIO" POST /api/auth/login "$LOGIN")" "login after re-create"
expect 200 "$(api "$AIO" GET "/api/environments/$ENV_ID/flags/smoke-flag" '')" "flag after re-create"
docker rm -f "$AIO" >/dev/null

# ---------------------------------------------------------------------------
log "external: separate PostgreSQL and Redis"
EXT_PG="$RUN_ID-pg"
EXT_REDIS="$RUN_ID-redis"
EXT="$RUN_ID-ext"
CONTAINERS+=("$EXT_PG" "$EXT_REDIS" "$EXT")
docker run -d --name "$EXT_PG" --network "$NET" \
  -e POSTGRES_USER=cgext -e POSTGRES_PASSWORD=ext-db-secret -e POSTGRES_DB=checkgate_ext \
  postgres:16 >/dev/null
docker run -d --name "$EXT_REDIS" --network "$NET" redis:7 >/dev/null
for _ in $(seq 1 60); do
  docker exec "$EXT_PG" pg_isready -U cgext -d checkgate_ext >/dev/null 2>&1 && break
  sleep 1
done

EXT_DB_URL="postgres://cgext:ext-db-secret@$EXT_PG:5432/checkgate_ext?sslmode=disable"
EXT_TOKEN="smoke-ext-setup-token-0123456789"
run_ext() {
  docker run -d --name "$EXT" --network "$NET" -p 127.0.0.1::3000 \
    -e DATABASE_URL="$EXT_DB_URL" -e REDIS_URL="redis://$EXT_REDIS:6379" \
    -e SESSION_SECRET="$SECRET" -e COOKIE_SECURE=false -e SETUP_TOKEN="$EXT_TOKEN" \
    "$IMAGE" >/dev/null
}
run_ext
wait_healthy "$EXT"
LOGS="$(docker logs "$EXT" 2>&1)"
grep -q "Mode: external" <<<"$LOGS" || fail "external mode not selected from DATABASE_URL"
grep -qF "postgres://***@$EXT_PG:5432/checkgate_ext?***" <<<"$LOGS" || fail "external DB target not logged (redacted)"
if grep -qF "ext-db-secret" <<<"$LOGS"; then fail "database password appears in the log"; fi
PROCS="$(process_names "$EXT")"
if grep -qxE "postgres|redis-server" <<<"$PROCS"; then fail "external mode started a bundled service"; fi
[ -z "$(docker exec "$EXT" ls -A /var/lib/postgresql/data)" ] || fail "external mode initialised the bundled data dir"

expect 200 "$(api "$EXT" POST /api/setup/complete "$SETUP" -H "X-Checkgate-Setup-Token: $EXT_TOKEN")" "external setup"
COMPLETE="$(docker exec "$EXT_PG" psql -U cgext -d checkgate_ext -tAc "SELECT value FROM settings WHERE key='setup_complete'")"
[ "$COMPLETE" = "true" ] || fail "setup did not land in the external database"
USERS="$(docker exec "$EXT_PG" psql -U cgext -d checkgate_ext -tAc "SELECT email FROM users")"
[ "$USERS" = "admin@smoke.test" ] || fail "admin not in the external database: $USERS"

log "external: container restart keeps data"
docker restart "$EXT" >/dev/null
wait_healthy "$EXT"
rm -f "$WORK/cookies-$EXT"
expect 200 "$(api "$EXT" POST /api/auth/login "$LOGIN")" "external login after restart"
expect_uid "$EXT" checkgate-serve checkgate
docker rm -f "$EXT" >/dev/null

log "external: whole container as a non-root user"
EXT_NR="$RUN_ID-ext-nonroot"
CONTAINERS+=("$EXT_NR")
docker run -d --name "$EXT_NR" --network "$NET" -p 127.0.0.1::3000 --user 10001:10001 \
  -e DATABASE_URL="$EXT_DB_URL" -e REDIS_URL="redis://$EXT_REDIS:6379" \
  -e SESSION_SECRET="$SECRET" -e COOKIE_SECURE=false -e SETUP_TOKEN="$EXT_TOKEN" \
  "$IMAGE" >/dev/null
wait_healthy "$EXT_NR"
expect_uid "$EXT_NR" checkgate-serve checkgate
[ "$(docker exec "$EXT_NR" id -u)" = "10001" ] || fail "docker exec does not default to the non-root user"
expect 200 "$(api "$EXT_NR" POST /api/auth/login "$LOGIN")" "non-root external login"
docker rm -f "$EXT_NR" >/dev/null

# ---------------------------------------------------------------------------
# expect_refusal DESCRIPTION EXPECTED_MESSAGE docker-run-args...
expect_refusal() {
  local what="$1" want="$2" out rc
  shift 2
  set +e
  out="$(timeout 120 docker run --rm --network "$NET" -e SESSION_SECRET="$SECRET" "$@" 2>&1)"
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || fail "$what: container started"
  [ "$rc" -ne 124 ] || fail "$what: container kept running"
  grep -qF "$want" <<<"$out" || fail "$what: missing '$want' in output: $out"
  log "refused as expected: $what"
}

log "misconfiguration is refused"
expect_refusal "external without DATABASE_URL" "external mode needs DATABASE_URL" \
  -e CHECKGATE_MODE=external -e REDIS_URL="redis://$EXT_REDIS:6379" "$IMAGE"
expect_refusal "external without REDIS_URL" "external mode needs REDIS_URL" \
  -e DATABASE_URL="$EXT_DB_URL" "$IMAGE"
expect_refusal "all-in-one with DATABASE_URL" "DATABASE_URL or REDIS_URL is set" \
  -e CHECKGATE_MODE=all-in-one -e DATABASE_URL="$EXT_DB_URL" "$IMAGE"
expect_refusal "unknown mode" "CHECKGATE_MODE must be" -e CHECKGATE_MODE=hybrid "$IMAGE"
expect_refusal "server without DATABASE_URL" "DATABASE_URL is not set" \
  --entrypoint checkgate-server "$IMAGE"
expect_refusal "server without REDIS_URL" "REDIS_URL is not set" \
  --entrypoint checkgate-server -e DATABASE_URL="$EXT_DB_URL" "$IMAGE"
expect_refusal "unreachable external database" "Failed to connect to PostgreSQL" \
  -e DATABASE_URL="postgres://cgext:ext-db-secret@$RUN_ID-missing:5432/checkgate_ext" \
  -e REDIS_URL="redis://$EXT_REDIS:6379" "$IMAGE"
expect_refusal "all-in-one as a non-root user" "must start as root" \
  --user 10001:10001 "$IMAGE"
expect_refusal "short SETUP_TOKEN" "SETUP_TOKEN must be at least" \
  -e DATABASE_URL="$EXT_DB_URL" -e REDIS_URL="redis://$EXT_REDIS:6379" -e SETUP_TOKEN=short "$IMAGE"

log "all smoke tests passed"
