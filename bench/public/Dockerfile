# syntax=docker/dockerfile:1
# Images for the public benchmark suite (bench/public/compose.yml).
#
#   server   the release server binary alone (PID 1 in its container, so the
#            load generators can read /proc/1 through a shared PID namespace)
#   loadgen  server/examples/throughput.rs
#   sdk      Node 22 + @checkgate/node built from this checkout + bench/public/sdk
#
# Build context: the repository root. Every base image is pinned by digest.

ARG RUST_IMAGE=rust:1.98-slim-bookworm@sha256:ff521445a372125ed4f76e1453a1f8098f2d05332d1601d30db1c1f62757e730
ARG NODE_IMAGE=node:22.23.4-bookworm-slim@sha256:5ec23b7c622b00547753bc6ffd78085dd868a68aeb41c8e0a1418f033e13d96d
ARG DEBIAN_IMAGE=debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc

FROM ${NODE_IMAGE} AS node

# ---------------------------------------------------------------------------
FROM ${RUST_IMAGE} AS build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
# Node and npm, for building the N-API addon with the project's own napi CLI.
COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=node /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -s /usr/local/lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
    && ln -s /usr/local/lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx

WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p server --bin server --example throughput \
    && mkdir -p /out \
    && cp target/release/server target/release/examples/throughput /out/
WORKDIR /src/sdks/nodejs
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    --mount=type=cache,target=/root/.npm \
    npm ci \
    && npx napi build --platform --release --js native-binding.js --dts native-binding.d.ts \
    && mkdir -p /out/sdk \
    && cp index.js cache-key.js native-binding.js package.json package-lock.json *.node /out/sdk/

# ---------------------------------------------------------------------------
FROM ${DEBIAN_IMAGE} AS server
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin checkgate \
    && mkdir -p /srv/public && chown checkgate /srv/public
COPY --from=build /out/server /usr/local/bin/checkgate-server
USER 10001
ENV PUBLIC_DIR=/srv/public
ENTRYPOINT ["/usr/local/bin/checkgate-server"]

# ---------------------------------------------------------------------------
FROM ${DEBIAN_IMAGE} AS loadgen
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/throughput /usr/local/bin/throughput
ENTRYPOINT ["/usr/local/bin/throughput"]

# ---------------------------------------------------------------------------
FROM ${NODE_IMAGE} AS sdk
COPY --from=build /out/sdk /sdk
RUN --mount=type=cache,target=/root/.npm cd /sdk && npm ci --omit=dev --ignore-scripts
COPY bench/public/sdk /bench
WORKDIR /bench
ENV CHECKGATE_SDK=/sdk/index.js
ENTRYPOINT ["node"]
