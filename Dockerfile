# =============================================================================
# Checkgate image
#
# Bundles: PostgreSQL + Redis + Checkgate server + React dashboard.
#
# One image, two modes (see docker/entrypoint-full.sh, CHECKGATE_MODE):
#   all-in-one  (default) starts the bundled PostgreSQL and Redis — demos,
#               staging, single-node self-hosting.
#   external    uses DATABASE_URL and REDIS_URL and starts neither bundled
#               service — managed databases, multiple replicas. Chosen
#               automatically when either URL is set.
# =============================================================================

# ---------------------------------------------------------------------------
# Stage 1 — build dashboard
# ---------------------------------------------------------------------------
FROM oven/bun:1.4.2-slim AS dashboard-builder

WORKDIR /app/dashboard
COPY dashboard/package.json dashboard/bun.lock* ./
RUN bun install --frozen-lockfile

COPY dashboard/ .
RUN bun run build
# → emits to ../server/public which maps to /app/server/public

# ---------------------------------------------------------------------------
# Stage 2 — build Rust server
# ---------------------------------------------------------------------------
FROM rust:1.98-slim-bookworm AS server-builder

RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Cache dependency layer
COPY Cargo.toml Cargo.lock ./
COPY core/Cargo.toml core/Cargo.toml
COPY server/Cargo.toml server/Cargo.toml
COPY sdks/nodejs/Cargo.toml sdks/nodejs/Cargo.toml
COPY sdks/web/Cargo.toml sdks/web/Cargo.toml
COPY sdks/flutter/dart/rust/Cargo.toml sdks/flutter/dart/rust/Cargo.toml
COPY sdks/react-native/Cargo.toml sdks/react-native/Cargo.toml

RUN mkdir -p core/src server/src sdks/nodejs/src sdks/web/src sdks/flutter/dart/rust/src sdks/react-native/src && \
    echo "fn main(){}" > server/src/main.rs && \
    echo "" > core/src/lib.rs && \
    echo "" > sdks/nodejs/src/lib.rs && \
    echo "" > sdks/web/src/lib.rs && \
    echo "" > sdks/flutter/dart/rust/src/lib.rs && \
    echo "" > sdks/react-native/src/lib.rs

RUN cargo build --release -p server 2>/dev/null || true

COPY core/ core/
COPY server/ server/

# Touch all .rs files so cargo detects the source change and recompiles the server crate.
# Without this, Docker COPY preserves the original mtime and cargo may skip recompilation.
RUN find core/src server/src -name "*.rs" | xargs touch && \
    cargo build --release -p server

# ---------------------------------------------------------------------------
# Stage 3 — all-in-one runtime
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim

# Install PostgreSQL, Redis, and runtime libs in one layer
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 \
        postgresql \
        postgresql-client \
        redis-server \
        curl && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=server-builder /app/target/release/server /usr/local/bin/checkgate-server
COPY --from=dashboard-builder /app/server/public ./public
COPY docker/entrypoint-full.sh /entrypoint.sh

# Processes never run as root. The server runs as `checkgate` (10001); in
# all-in-one mode the bundled PostgreSQL and Redis run as their own package
# users. The entrypoint starts as root only to prepare volumes for those
# services and then drops privileges for every long-running process; external
# mode also runs with `--user 10001:10001` (or Kubernetes runAsNonRoot), in
# which case nothing at all runs as root.
RUN sed -i 's/\r//' /entrypoint.sh && chmod +x /entrypoint.sh && \
    groupadd --system --gid 10001 checkgate && \
    useradd --system --uid 10001 --gid checkgate --home-dir /var/lib/checkgate \
        --no-create-home --shell /usr/sbin/nologin checkgate && \
    mkdir -p /var/lib/postgresql/data /var/log /var/lib/checkgate /var/lib/redis && \
    chown checkgate:checkgate /var/lib/checkgate && chmod 700 /var/lib/checkgate && \
    chown -R postgres:postgres /var/lib/postgresql && \
    chown redis:redis /var/lib/redis && \
    touch /var/log/postgresql.log /var/log/redis.log && \
    chown postgres:postgres /var/log/postgresql.log && \
    chown redis:redis /var/log/redis.log

EXPOSE 3000

# Liveness only: a database outage must not mark the container unhealthy.
# Orchestrators should route traffic by /health/ready (see docs/operations).
HEALTHCHECK --interval=30s --timeout=10s --start-period=10s --retries=3 \
  CMD curl -fsS http://localhost:3000/health/live >/dev/null || exit 1

# POSTGRES_USER / POSTGRES_PASSWORD / POSTGRES_DB default to `checkgate` in
# the entrypoint (all-in-one mode only), so no credential is baked into the
# image's environment.
ENV PUBLIC_DIR=/app/public \
    SETUP_TOKEN_FILE=/var/lib/checkgate/setup-token \
    PGDATA=/var/lib/postgresql/data

ENTRYPOINT ["/entrypoint.sh"]
