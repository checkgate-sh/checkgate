#!/usr/bin/env bash
# Starts a local, development-only Checkgate and waits until it is ready.
#
#   ./quickstart/start.sh            then finish setup at http://localhost:3000/setup
#   ./quickstart/start.sh --seed     also create an admin, a sample flag and print an SDK key
#   ./quickstart/start.sh --down     stop and delete the quickstart data
#
# Secrets are generated per machine into quickstart/.env (git-ignored). The
# setup wizard still requires the one-time setup token; --seed uses it the
# same way the wizard does. Never use these settings in production.
set -euo pipefail
cd "$(dirname "$0")"

PORT="${CHECKGATE_PORT:-3000}"
URL="http://localhost:${PORT}"
compose() { docker compose --env-file .env -f compose.yml "$@"; }

if ! command -v docker >/dev/null 2>&1; then
  echo "docker not found: install Docker Desktop or Docker Engine first." >&2
  exit 1
fi
if ! docker info >/dev/null 2>&1; then
  echo "The Docker daemon is not running: start Docker and run this again." >&2
  exit 1
fi

if [ "${1:-}" = "--down" ]; then
  [ -f .env ] && compose down -v
  rm -f .env
  echo "Quickstart stopped and its data deleted."
  exit 0
fi

rand() { openssl rand -hex 32 2>/dev/null || head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n'; }
if [ ! -f .env ]; then
  umask 077
  {
    echo "SESSION_SECRET=$(rand)"
    echo "SETUP_TOKEN=$(rand)"
    echo "POSTGRES_PASSWORD=$(rand)"
    echo "CHECKGATE_PORT=${PORT}"
  } > .env
fi
# shellcheck disable=SC1091
. ./.env

if curl -fsS "http://127.0.0.1:${PORT}/health/live" >/dev/null 2>&1 && \
   [ -z "$(compose ps -q 2>/dev/null)" ]; then
  echo "Port ${PORT} is already in use by something else: set CHECKGATE_PORT=3001 (or another free port)." >&2
  exit 1
fi

echo "Starting Checkgate on ${URL} ..."
compose up -d --quiet-pull
for _ in $(seq 1 120); do
  if curl -fsS "http://127.0.0.1:${PORT}/health/ready" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
if ! curl -fsS "http://127.0.0.1:${PORT}/health/ready" >/dev/null 2>&1; then
  echo "Checkgate did not become ready within 2 minutes. Logs:" >&2
  compose logs --tail 50 >&2
  exit 1
fi
echo "Ready."

if [ "${1:-}" = "--seed" ]; then
  CHECKGATE_URL="${URL}" SETUP_TOKEN="${SETUP_TOKEN}" node seed.mjs
else
  echo
  echo "Finish setup at ${URL}/setup with this one-time setup token:"
  echo "  ${SETUP_TOKEN}"
fi
