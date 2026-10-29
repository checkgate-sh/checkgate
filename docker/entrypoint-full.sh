#!/usr/bin/env bash
# Checkgate image entrypoint.
#
# The image runs in one of two modes, chosen by CHECKGATE_MODE:
#
#   all-in-one  Start the bundled PostgreSQL and Redis, then the server, wired
#               to them. Data lives in the /var/lib/postgresql/data volume.
#   external    Start only the server, using DATABASE_URL and REDIS_URL as
#               given. The bundled PostgreSQL and Redis are never started.
#
# When CHECKGATE_MODE is unset, the mode is external if DATABASE_URL or
# REDIS_URL is set, and all-in-one otherwise. Connection variables are never
# overwritten, and a half-specified configuration stops the container instead
# of falling back to the bundled services.
set -euo pipefail

die() {
  echo "[error] $*" >&2
  exit 1
}

# Strip credentials before a URL reaches the log: the userinfo part, and the
# whole query string (libpq-style URLs may carry ?password=…).
redact() {
  printf '%s' "$1" | sed -E 's#(://)[^@/]*@#\1***@#; s#\?.*#?***#'
}

MODE="${CHECKGATE_MODE:-}"
if [ -z "$MODE" ]; then
  if [ -n "${DATABASE_URL:-}" ] || [ -n "${REDIS_URL:-}" ]; then
    MODE=external
  else
    MODE=all-in-one
  fi
fi

case "$MODE" in
  external)
    [ -n "${DATABASE_URL:-}" ] || die "external mode needs DATABASE_URL (postgres://user:password@host:5432/db). Set it, or use CHECKGATE_MODE=all-in-one with neither DATABASE_URL nor REDIS_URL set."
    [ -n "${REDIS_URL:-}" ] || die "external mode needs REDIS_URL (redis://host:6379). Set it, or use CHECKGATE_MODE=all-in-one with neither DATABASE_URL nor REDIS_URL set."
    echo "[init] Mode: external — bundled PostgreSQL and Redis are not started."
    echo "[init] PostgreSQL: $(redact "$DATABASE_URL")"
    echo "[init] Redis:      $(redact "$REDIS_URL")"
    export PUBLIC_DIR="${PUBLIC_DIR:-/app/public}"
    exec checkgate-server
    ;;
  all-in-one)
    if [ -n "${DATABASE_URL:-}" ] || [ -n "${REDIS_URL:-}" ]; then
      die "CHECKGATE_MODE=all-in-one runs the bundled PostgreSQL and Redis, but DATABASE_URL or REDIS_URL is set. Unset them, or use CHECKGATE_MODE=external."
    fi
    echo "[init] Mode: all-in-one — starting bundled PostgreSQL and Redis."
    ;;
  *)
    die "CHECKGATE_MODE must be 'all-in-one' or 'external', got '$MODE'."
    ;;
esac

PG_DATA="${PGDATA:-/var/lib/postgresql/data}"
PG_USER="${POSTGRES_USER:-checkgate}"
PG_PASSWORD="${POSTGRES_PASSWORD:-checkgate}"
PG_DB="${POSTGRES_DB:-checkgate}"

# Locate the versioned PostgreSQL bin dir (e.g. /usr/lib/postgresql/15/bin)
PG_BIN=$(find /usr/lib/postgresql -name "initdb" -type f 2>/dev/null | head -1 | xargs -r dirname)
if [ -z "$PG_BIN" ]; then
  die "Could not locate PostgreSQL binaries. Is postgresql installed?"
fi
export PATH="$PG_BIN:$PATH"

if [ "${PG_PASSWORD}" = "checkgate" ]; then
  echo "[warn] POSTGRES_PASSWORD is set to the default value 'checkgate'. Set a strong password in production."
fi

# ---------------------------------------------------------------------------
# 1. Initialise PostgreSQL data directory (first boot only)
# ---------------------------------------------------------------------------
if [ ! -f "$PG_DATA/PG_VERSION" ]; then
  echo "[init] Initialising PostgreSQL data directory..."
  PGPASS_FILE=$(mktemp)
  printf '%s' "$PG_PASSWORD" > "$PGPASS_FILE"
  chown postgres:postgres "$PGPASS_FILE"
  su -s /bin/sh postgres -c "initdb -D '$PG_DATA' --username='$PG_USER' --pwfile='$PGPASS_FILE' --auth-host=md5 --auth-local=trust"
  rm -f "$PGPASS_FILE"
  echo "[init] PostgreSQL initialised."
fi

# ---------------------------------------------------------------------------
# 2. Start PostgreSQL
# ---------------------------------------------------------------------------
echo "[init] Starting PostgreSQL..."
su -s /bin/sh postgres -c "pg_ctl -D '$PG_DATA' -l /var/log/postgresql.log start -w"

# Ensure database exists (connect to 'postgres' which always exists after initdb)
su -s /bin/sh postgres -c "psql -U '$PG_USER' -d postgres -tc \"SELECT 1 FROM pg_database WHERE datname='$PG_DB'\" | grep -q 1 || psql -U '$PG_USER' -d postgres -c \"CREATE DATABASE \\\"$PG_DB\\\";\""
echo "[init] PostgreSQL ready."

# ---------------------------------------------------------------------------
# 3. Start Redis (with optional password)
# ---------------------------------------------------------------------------
echo "[init] Starting Redis..."
REDIS_ARGS="--daemonize yes --logfile /var/log/redis.log --dir /var/lib/redis --appendonly yes"
if [ -n "${REDIS_PASSWORD:-}" ]; then
  REDIS_ARGS="${REDIS_ARGS} --requirepass ${REDIS_PASSWORD}"
  echo "[init] Redis password authentication enabled."
else
  echo "[warn] REDIS_PASSWORD is not set. Redis is running without authentication."
fi
# shellcheck disable=SC2086
redis-server ${REDIS_ARGS}
echo "[init] Redis ready."

# ---------------------------------------------------------------------------
# 4. Point the server at the bundled services
# ---------------------------------------------------------------------------
export DATABASE_URL="postgres://${PG_USER}:${PG_PASSWORD}@127.0.0.1/${PG_DB}"
if [ -n "${REDIS_PASSWORD:-}" ]; then
  export REDIS_URL="redis://:${REDIS_PASSWORD}@127.0.0.1:6379"
else
  export REDIS_URL="redis://127.0.0.1:6379"
fi
export PUBLIC_DIR="${PUBLIC_DIR:-/app/public}"

echo "[init] Starting Checkgate server..."
exec checkgate-server
