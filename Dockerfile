# =============================================================================
# Checkgate — all-in-one image
#
# Bundles: PostgreSQL 16 + Redis 7 + Checkgate server + React dashboard.
# Perfect for demos, staging, or single-node self-hosted deployments.
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

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

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

RUN sed -i 's/\r//' /entrypoint.sh && chmod +x /entrypoint.sh && \
    mkdir -p /var/lib/postgresql/data /var/log && \
    chown -R postgres:postgres /var/lib/postgresql && \
    touch /var/log/postgresql.log /var/log/redis.log && \
    chown postgres:postgres /var/log/postgresql.log /var/log/redis.log

EXPOSE 3000

# Allow orchestrators to monitor container health
HEALTHCHECK --interval=30s --timeout=10s --start-period=10s --retries=3 \
  CMD curl -f http://localhost:3000/health || exit 1

ENV PUBLIC_DIR=/app/public \
    PGDATA=/var/lib/postgresql/data \
    POSTGRES_USER=checkgate \
    POSTGRES_PASSWORD=checkgate \
    POSTGRES_DB=checkgate

ENTRYPOINT ["/entrypoint.sh"]
