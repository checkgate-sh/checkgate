# Deployment readiness: v0.1.31

Supported deployment methods, as shipped: **one Docker image** (`Dockerfile`)
with two modes (`all-in-one` with bundled PostgreSQL and Redis, `external`
with `DATABASE_URL` and `REDIS_URL`), and `docker-compose.yml` for all-in-one.
No Kubernetes manifests or Helm charts are shipped, and none were added. The
Kubernetes operator in `integrations/` manages flags, not the server.

## Verification

All results are from 2026-11-18 on Docker Desktop 29.7.2, image built from
the v0.1.31 candidate (`checkgate:v0131-rc`).

| Check | Result | Evidence |
|---|---|---|
| Fresh install, all-in-one: first boot, generated setup token (0600 file, never logged), setup, login | pass | `scripts/docker-smoke.sh`, all 19 scenarios |
| Persistent storage: data survives a restart and a re-created container on the same volume | pass | smoke |
| Restart after a killed container (stale `postmaster.pid`) | pass | smoke |
| External mode with separate PostgreSQL and Redis, including running the whole container as a non-root user | pass | smoke |
| Configuration validation: 9 half-specified or invalid configurations refused with a clear error | pass | smoke ("refused as expected: …") |
| No service runs as root (server UID 10001, `postgres`, `redis`) | pass | smoke `expect_uid` |
| Health: `/health/live`, `/health/ready`, Docker `HEALTHCHECK` | pass | smoke, `tests/lifecycle.rs` |
| Graceful termination of the server (drain, streams closed, exit 0) | pass | `tests/lifecycle.rs` (8/8) |
| **Clean shutdown of the bundled PostgreSQL and Redis on `docker stop`** | **v0.1.30: fail (D1). v0.1.31: pass** | below |
| Linux process metrics exposed on `/metrics` | pass | in container: `process_resident_memory_bytes 13283328`, `process_open_fds 17`, `process_threads 9` |
| Upgrade from v0.1.30, rolling upgrade, rollback | pass | UPGRADE-AND-ROLLBACK.md |

## D1: the bundled PostgreSQL was killed on every container stop (fixed)

**Cause.** In all-in-one mode the entrypoint `exec`'d the server, making it
PID 1. PostgreSQL and Redis ran as daemonized children. On `docker stop`, the
server drained and exited (correctly, in milliseconds), and the kernel then
SIGKILLed every remaining process in the container.

**Reproduced** with the v0.1.30 image (`results/docker-stop/v0.1.30.txt`).
After one ordinary `docker stop` / `docker start`, PostgreSQL logged:

```
LOG:  database system was interrupted; last known up at …
LOG:  database system was not properly shut down; automatic recovery in progress
LOG:  redo starts at 0/1512418
```

Committed data was safe, since WAL replay recovers it, but every restart ran
crash recovery, and its duration grows with the WAL since the last
checkpoint. Redis (`appendonly yes`, default `appendfsync everysec`) could lose
its last second of writes. The Redis data here is the pub/sub channel and its
append-only file, so no flag data is at stake.

**Fix** (`docker/entrypoint-full.sh`, `supervise_server`). In all-in-one mode
the shell stays PID 1. It forwards SIGTERM/SIGINT to the server, waits for the
server to exit, sends Redis SIGTERM (which flushes its AOF), and runs
`pg_ctl stop -m fast`, then exits with the server's status. External mode
still `exec`s the server. `docker-compose.yml` sets `stop_grace_period: 30s`,
because Docker's default 10 s is below the server's own
`SHUTDOWN_GRACE_PERIOD_SECS` (20 s).

**Verified** with the v0.1.31 image (`results/docker-stop/v0.1.31.txt`):

```
[shutdown] Server exited with status 0; stopping Redis and PostgreSQL.
[shutdown] Bundled services stopped.
LOG:  received fast shutdown request
LOG:  database system is shut down
… (restart)
LOG:  database system was shut down at …          ← clean, no recovery
```

`docker stop` took 0.6 s, exit code 0.

**Regression test.** `scripts/docker-smoke.sh` now fails the run if the
bundled PostgreSQL was not asked to shut down, or ran crash recovery, after a
container restart. It **fails on the v0.1.30 image** ("PostgreSQL was not
asked to shut down on container stop") and passes on v0.1.31. CI runs it on
every PR (job `docker-smoke`).

**Trade-off.** The supervising shell runs as root for the container's
lifetime, because it must stop PostgreSQL and Redis under their own users.
It only waits and forwards signals; it has no network exposure and processes
no input. All three services still run as non-root users.

## Other fixes in this area

- `scripts/docker-smoke.sh` now runs on macOS's bash 3.2. Empty-array
  expansions under `set -u` aborted it before, so maintainers on Macs could
  not run the suite locally; CI's bash 5 was unaffected.

## Resource limits and defaults (unchanged, verified present)

| Setting | Default | Purpose |
|---|---|---|
| `SSE_MAX_CONNECTIONS` | 10 000 | Per-instance stream cap (503 + `Retry-After` beyond) |
| `SSE_BOOTSTRAP_CONCURRENCY` | half of `DB_MAX_CONNECTIONS` | Reconnect waves cannot take the whole DB pool |
| API / ingest body limits | 64 KiB / 256 KiB | Request size |
| Configuration limits (new) | see UPGRADE-AND-ROLLBACK.md | Bound derived (expanded) sizes |
| `SHUTDOWN_GRACE_PERIOD_SECS` | 20 | Keep the orchestrator's stop timeout above it (compose: 30 s) |
| `COOKIE_SECURE` | `true` in the binary; the compose example sets `false` for plain-HTTP localhost | Set `true` (or unset) behind TLS |

## Remaining limitations

- `docker-compose.yml` defaults `POSTGRES_PASSWORD` to `checkgate` and leaves
  `REDIS_PASSWORD` unset. Both services are reachable only inside the
  container in all-in-one mode, but operators should set both. This is
  unchanged and documented in `docs/self-hosting.md`.
- No multi-instance compose or Kubernetes example ships with the repository.
  Multi-instance behaviour is covered by `tests/reliability.rs`, not by a
  deployment example.
