# Release A/B scripts

Local before/after comparisons between two builds, used to produce the
evidence in `docs/releases/<version>/`. They complement the public benchmark
suite (`bench/public/`), which measures one build under fixed resource limits
in Docker; these compare two builds side by side on one host.

| Script | Compares | Needs |
|---|---|---|
| `node-eval-ab.sh` | Node SDK evaluation latency (`bench/public/sdk/eval.mjs`), alternating runs | Both SDKs' addons built (`npm run build` in `sdks/nodejs`) |
| `snapshot-ab.sh` | `GET /flags/snapshot` throughput, latency and server CPU per request | Release server binaries, the `throughput` example, PostgreSQL and Redis containers |
| `node-differential.cjs` | Every `isEnabled`/`getValue`/`getVariant` result of two Node addons over 300 generated flags × 400 generated contexts | Both addons built |
| `upgrade-rollback.sh` | Upgrade, mixed-version (rolling) and rollback on one database | Release server binaries of both versions, the same containers |

The baseline is usually a detached worktree of the previous release tag, built
into its **own** `CARGO_TARGET_DIR`. Worktrees that share a target dir reuse
each other's artifacts, so an A/B can silently measure one build twice.

```bash
git worktree add --detach ../checkgate-base v0.1.30
(cd ../checkgate-base && CARGO_TARGET_DIR=/tmp/cg-base-target cargo build --release -p server --bin server)
cargo build --release -p server --bin server --example throughput

docker run -d --name cg-gate-pg -p 55432:5432 -e POSTGRES_PASSWORD=postgres postgres:16-alpine
docker run -d --name cg-gate-redis -p 56379:6379 redis:7-alpine

BASE_BIN=/tmp/cg-base-target/release/server bench/ab/snapshot-ab.sh
OLD_BIN=/tmp/cg-base-target/release/server bench/ab/upgrade-rollback.sh
BASE_SDK=../checkgate-base/sdks/nodejs/index.js bench/ab/node-eval-ab.sh
```

Output goes to `bench/ab/out/` (ignored by Git).
