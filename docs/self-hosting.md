---
title: "Self-Hosting Checkgate — Docker & AWS Deployment Guide"
description: "Deploy Checkgate on your own infrastructure: the all-in-one Docker image, or the same image against your own PostgreSQL and Redis. Covers local development, Docker Compose, production configuration, health checks, rate limits, reverse proxies and upgrades."
---

# Self-Hosting

Checkgate is designed to be self-hosted. One Docker image covers two deployment
modes:

| Mode | What runs in the container | Use it for |
|------|----------------------------|------------|
| **All-in-one** (default) | PostgreSQL + Redis + server + dashboard | Local evaluation, demos, staging, single-node installs |
| **External services** | Server + dashboard only, using your `DATABASE_URL` and `REDIS_URL` | Managed databases (RDS, Cloud SQL, ElastiCache…), several replicas, production |

The image picks the mode from `CHECKGATE_MODE` (`all-in-one` or `external`).
When that is unset, it runs **external** if `DATABASE_URL` or `REDIS_URL` is set
and **all-in-one** otherwise. It never overrides connection strings you pass,
and it refuses to start on a half-specified configuration — for example
`DATABASE_URL` without `REDIS_URL` — rather than quietly using the bundled
services.

## Docker Compose (all-in-one)

Run the published release image with Docker Compose. No repository clone or local build is needed.

Create a deployment directory with a `.env` file (the server refuses to start
without `SESSION_SECRET`):

```bash
SESSION_SECRET=$(openssl rand -hex 32)      # Required
POSTGRES_PASSWORD=strong-db-password        # Replaces default "checkgate"
SETUP_TOKEN=$(openssl rand -hex 32)         # One-time token for the setup wizard
COOKIE_SECURE=false                         # true once served over HTTPS
```

Write the values themselves into the file (run the `openssl` commands and
paste their output); Compose does not execute `$(…)` in `.env` files.

Save the following as `compose.yml`:

```yaml
services:
  checkgate:
    image: ghcr.io/checkgate-sh/checkgate:latest
    container_name: checkgate
    ports:
      - "3000:3000"
    environment:
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:-checkgate}
      SESSION_SECRET: ${SESSION_SECRET:?set SESSION_SECRET in .env}
      SETUP_TOKEN: ${SETUP_TOKEN:-}
      COOKIE_SECURE: ${COOKIE_SECURE:-false}
    volumes:
      - checkgate_pgdata:/var/lib/postgresql/data
      - checkgate_redis:/var/lib/redis
    restart: unless-stopped
    stop_grace_period: 30s   # longer than the server's 20 s shutdown grace

volumes:
  checkgate_pgdata:
  checkgate_redis:
```

Pull the image and start Checkgate:

```bash
docker compose pull
docker compose up -d
```

This starts a single container that includes:
- **PostgreSQL** — persistent flag and user storage, in the `checkgate_pgdata` volume
- **Redis** — pub/sub for real-time sync
- **Checkgate server** — REST API + SSE stream on port `3000`
- **React dashboard** — served at `http://localhost:3000`

Open the dashboard and enter the setup token (see [First-run setup](#first-run-setup)).
Keep the `checkgate_pgdata` volume: it holds all of your data and survives
container restarts, re-creation and image upgrades.

## External PostgreSQL and Redis

Pass both connection strings; the container then starts only the server and
dashboard:

```bash
docker run -d --name checkgate -p 3000:3000 \
  -e DATABASE_URL="postgres://checkgate:password@your-postgres:5432/checkgate" \
  -e REDIS_URL="redis://your-redis:6379" \
  -e SESSION_SECRET="$(openssl rand -hex 32)" \
  -e SETUP_TOKEN="$(openssl rand -hex 32)" \
  -e COOKIE_SECURE=true \
  ghcr.io/checkgate-sh/checkgate:<version>
```

Or with Compose:

```yaml
services:
  checkgate:
    image: ghcr.io/checkgate-sh/checkgate:<version>
    ports:
      - "3000:3000"
    environment:
      CHECKGATE_MODE: external          # optional — implied by the URLs below
      DATABASE_URL: ${DATABASE_URL}
      REDIS_URL: ${REDIS_URL}
      SESSION_SECRET: ${SESSION_SECRET}
      SETUP_TOKEN: ${SETUP_TOKEN}
      COOKIE_SECURE: "true"
    restart: unless-stopped
    stop_grace_period: 30s   # longer than the server's 20 s shutdown grace
```

Requirements and behaviour:

- **Both** `DATABASE_URL` and `REDIS_URL` are required. If either is missing,
  the container exits with an error naming it. It never falls back to the
  bundled services or to `localhost`.
- The database must already exist; Checkgate creates and migrates its tables on
  startup, so the role needs permission to create tables in it. Tested with
  PostgreSQL 15 (bundled in the image) and 16 (CI).
- The container logs the target it connects to with credentials removed
  (`postgres://***@your-postgres:5432/checkgate`). If the database is
  unreachable the container exits instead of running half-configured.
- Setting `CHECKGATE_MODE=all-in-one` together with either URL is a
  configuration error and is refused.

## Local development

Run PostgreSQL and Redis in containers and the server from source:

```bash
docker run -d --name cg-dev-pg -e POSTGRES_USER=checkgate -e POSTGRES_PASSWORD=password \
  -e POSTGRES_DB=checkgate -p 5432:5432 postgres:16
docker run -d --name cg-dev-redis -p 6379:6379 redis:7

cargo run -p server            # http://localhost:3000
cd dashboard && bun install && bun run dev
```

Debug builds default `DATABASE_URL` to `postgres://checkgate:password@localhost/checkgate`,
`REDIS_URL` to `redis://localhost:6379` and `SESSION_SECRET` to an insecure
development key, and log a warning for each. **Release builds — including the
Docker image — have no such defaults** and refuse to start without them. With
`SETUP_TOKEN` unset, the first start writes the setup token to
`./checkgate-setup-token` (git-ignored); or export `SETUP_TOKEN` yourself.

## Production configuration

- Pin a version tag instead of `:latest`.
- Set `SESSION_SECRET` (≥ 32 random characters) and `SETUP_TOKEN`, and keep
  them in your secret store rather than in the image or compose file.
- Serve Checkgate over HTTPS and set `COOKIE_SECURE=true`.
- Behind a reverse proxy or load balancer, set `TRUSTED_PROXIES` to its
  addresses (see [Behind a reverse proxy](#behind-a-reverse-proxy-or-load-balancer)),
  or every client is rate-limited as the proxy.
- For several replicas, use external mode: all replicas must share one
  PostgreSQL database, one Redis, the same `SESSION_SECRET` and the same
  `SETUP_TOKEN`.
- Size `DB_MAX_CONNECTIONS` × replicas below your database's connection limit.
- Probe `/health/live` for liveness and `/health/ready` for readiness, and set
  `SHUTDOWN_DRAIN_DELAY_SECS` / `SHUTDOWN_GRACE_PERIOD_SECS` below your
  orchestrator's termination grace period (see
  [Deployment, failure and recovery](./operations/failure-recovery.md)).
- Enable `METRICS_ADDR` on a private interface and scrape it
  (see [Observability](./operations/observability.md)).
- In external mode, run the container as `--user 10001:10001` (Kubernetes:
  `runAsNonRoot`). All-in-one mode starts as root only to prepare its volumes
  and runs every process unprivileged.
- Back up PostgreSQL (see [Backup](#backup)); Redis holds no durable state.

## Docker Images

One image is published per release:

| Image | Description |
|-------|-------------|
| `ghcr.io/checkgate-sh/checkgate:latest` | All-in-one or external mode (see above) |
| `ghcr.io/checkgate-sh/checkgate:<version>` | Pinned release version (use the version tag without its leading `v`) |

The image is multi-architecture: `linux/amd64` and `linux/arm64`. Docker selects the matching architecture automatically. For production, replace `:latest` in `compose.yml` with a published version tag to control upgrades.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `CHECKGATE_MODE` | _(auto)_ | Image only: `all-in-one` or `external`. Unset: external if `DATABASE_URL` or `REDIS_URL` is set. |
| `SESSION_SECRET` | _(none — required)_ | Secret for encrypting session cookies. A random ≥32-char string. Release builds refuse to start without it. |
| `SETUP_TOKEN` | _(generated)_ | One-time token for the setup wizard, ≥16 chars. Unset: each instance generates one (see [First-run setup](#first-run-setup)). |
| `SETUP_TOKEN_FILE` | `checkgate-setup-token` (image: `/var/lib/checkgate/setup-token`) | Where a generated setup token is written. |
| `DATABASE_URL` | _(none — required)_ | PostgreSQL connection string. Set by the image in all-in-one mode; required in external mode and for the bare binary. |
| `REDIS_URL` | _(none — required)_ | Redis connection string. Same rules as `DATABASE_URL`. |
| `POSTGRES_PASSWORD` | `checkgate` | All-in-one only: password for the bundled PostgreSQL |
| `POSTGRES_USER` | `checkgate` | All-in-one only: username for the bundled PostgreSQL |
| `POSTGRES_DB` | `checkgate` | All-in-one only: database name for the bundled PostgreSQL |
| `REDIS_PASSWORD` | _(unset)_ | All-in-one only: password for the bundled Redis |
| `COOKIE_SECURE` | `true` | Set `false` for local HTTP development. Controls the `Secure` cookie flag. |
| `TRUSTED_PROXIES` | _(none)_ | IPs/CIDRs whose forwarding header is believed (see [Rate limits](#rate-limits)) |
| `TRUSTED_PROXY_HEADER` | `x-forwarded-for` | Header trusted proxies set: `x-forwarded-for`, or `forwarded` (RFC 7239). Only one is ever read. |
| `RATE_LIMIT_AUTH_PER_MINUTE` | `20` | Per-client login/setup budget; `0` disables |
| `RATE_LIMIT_API_PER_MINUTE` | `300` | Per-client REST API budget; `0` disables |
| `RATE_LIMIT_SDK_PER_MINUTE` | `600` | Per-client `/flags/snapshot` budget; `0` disables |
| `RATE_LIMIT_SSE_PER_MINUTE` | `120` | Per-client SSE connection budget; `0` disables |
| `RATE_LIMIT_INGEST_PER_MINUTE` | `600` | Per-client impression/event budget; `0` disables |
| `RATE_LIMIT_INGEST_KEY_PER_MINUTE` | `20000` | Ingest requests per SDK key (or user), shared by all instances; `0` disables (see [Per-key ingest quota](#per-key-ingest-quota)) |
| `RATE_LIMIT_INGEST_ROWS_PER_MINUTE` | `60000` | Impression and event rows per SDK key (or user), shared by all instances; `0` disables, otherwise at least `500` |
| `ANALYTICS_RETENTION_DAYS` | `90` | Days impressions and events are kept (0–3650); `0` keeps them forever (see [Analytics retention](#analytics-retention)) |
| `ANALYTICS_RETENTION_INTERVAL_SECS` | `3600` | Minimum time between retention passes across all instances (1–604800) |
| `ANALYTICS_RETENTION_BATCH_SIZE` | `5000` | Rows examined per retention batch (100–100000) |
| `SDK_KEY` | _(unset)_ | Legacy env var: adds an extra accepted SDK key without a DB entry |
| `PUBLIC_DIR` | `public` | Directory to serve dashboard static files from |
| `PORT` | `3000` | Port the server listens on |
| `DB_MAX_CONNECTIONS` | `10` | PostgreSQL connection pool size |
| `RUST_LOG` | `info` | Log level (`error`, `warn`, `info`, `debug`, `trace`) |
| `METRICS_ADDR` | _(unset: off)_ | Address for the Prometheus `/metrics` listener, e.g. `0.0.0.0:9464`. Never on `PORT`. See [Observability](./operations/observability.md). |
| `SHUTDOWN_DRAIN_DELAY_SECS` | `0` | After SIGTERM: readiness fails, streams stay open this long for load balancers to react |
| `SHUTDOWN_GRACE_PERIOD_SECS` | `20` | After SIGTERM: hard limit before the process exits |
| `READY_REQUIRES_REDIS` | `false` | Fail `/health/ready` while the Redis subscription is down |
| `SSE_MAX_CONNECTIONS` | `10000` | Open SSE streams per instance; `0` = unlimited. Beyond it: `503` + `Retry-After` |
| `SSE_BOOTSTRAP_CONCURRENCY` | `DB_MAX_CONNECTIONS / 2` | Concurrent SSE bootstraps and snapshot reads |
| `SSE_BOOTSTRAP_QUEUE_TIMEOUT_SECS` | `10` | Wait for a bootstrap slot before `503` |
| `SSE_HEARTBEAT_INTERVAL_SECS` | `15` | Quiet time before a stream gets a `heartbeat` event (1–300). SDKs reconnect after 3 missed intervals; keep it below your proxy's idle timeout |
| `SSE_RESYNC_SPREAD_MS` | `5000` | Window over which resynced streams are closed |
| `TCP_USER_TIMEOUT_SECS` | `60` | Linux: abort connections whose data stays unacknowledged this long; `0` disables |
| `REDIS_CHANNEL` | `checkgate_updates` | Redis Pub/Sub channel for change events; also prefixes the ingest-quota keys, so deployments sharing one Redis must use different values |
| `REDIS_PING_INTERVAL_SECS` | `10` | Liveness ping on the Redis subscription |
| `SYNC_RECONCILE_INTERVAL_SECS` | `30` | Reconciliation against PostgreSQL; `0` disables (see [Reliability](./operations/reliability.md)) |
| `SYNC_RECONCILE_GRACE_SECS` | `10` | How long a change may stay unrelayed before it is repaired |

### Security

Always set `SESSION_SECRET` in production — it is what keeps session cookies
from being forged. Release builds refuse to start without it.

```bash
# Generate a secure session secret
openssl rand -hex 32
```

SDK keys are managed through the dashboard Settings page. The first key is auto-generated on first boot. Additional keys can be created and revoked without restarting the server.

SDK keys ship inside browser and mobile apps, so treat them as **public**: a
key can only stream, poll and read the flags of its own environment and report
impressions and events for it. What someone holding a key can send is bounded
by the [per-key ingest quota](#per-key-ingest-quota), the 500-row and 256 KiB
limits per request and the [per-address limits](#rate-limits), and what is
stored expires with [analytics retention](#analytics-retention). Revoke a key
that is being abused; its open streams close on every instance.

## First-run setup

A fresh installation is claimed by whoever completes the setup wizard, so the
wizard requires a one-time **setup token**. This keeps an internet-reachable
server from being taken over by the first stranger to open it.

- **Choose the token yourself (recommended for production and required for
  read-only filesystems):** set `SETUP_TOKEN` to a random string of at least 16
  characters (`openssl rand -hex 32`). With several replicas, give them all the
  same value.
- **Or let the server generate one:** with `SETUP_TOKEN` unset, every instance
  writes a random token to `SETUP_TOKEN_FILE` (default `checkgate-setup-token`
  in its working directory; `/var/lib/checkgate/setup-token` in the Docker
  image) with mode `0600`, and logs the file's path — never the token. Any
  instance accepts any instance's token. Read it with, for example:

  ```bash
  docker exec checkgate cat /var/lib/checkgate/setup-token
  ```

Paste the token into the first step of the wizard. It is sent in the
`X-Checkgate-Setup-Token` header, never in a URL. Completing setup invalidates
every setup token and the instance deletes its token file; other instances
delete theirs on their next start.

**Upgrading:** installations that have already completed setup are not
affected. An installation that was never set up needs the token from now on.
No database migration is involved — generated token hashes are stored in the
existing `settings` table.

## Rate limits

Requests are limited per client IP, with an independent budget per class of
endpoint, so SDK traffic cannot lock operators out of the dashboard and a
password-guessing run cannot exhaust the SDK budget. `/health`, `/health/live`
and `/health/ready` are never limited, nor are dashboard files. Opening an SSE stream counts once; events delivered on an open stream
are never counted.

| Variable | Default (per minute) | Applies to |
|----------|----------------------|------------|
| `RATE_LIMIT_AUTH_PER_MINUTE` | `20` | `POST /api/auth/login`, `/api/setup/*` |
| `RATE_LIMIT_API_PER_MINUTE` | `300` | all other `/api/*` routes |
| `RATE_LIMIT_SDK_PER_MINUTE` | `600` | `GET /flags/snapshot` |
| `RATE_LIMIT_SSE_PER_MINUTE` | `120` | opening `GET /stream` |
| `RATE_LIMIT_INGEST_PER_MINUTE` | `600` | impression and event ingestion |

Set a variable to `0` to disable that class (for example, when a gateway in
front of Checkgate already limits it). Many SDK instances behind one NAT or
egress IP share a budget; raise the SDK, SSE and ingest limits accordingly.
Ingestion also has a [quota per SDK key](#per-key-ingest-quota), which holds
however many addresses use the key.
Limiter state holds only recently active clients (IPv6 clients are grouped by
/64) and is pruned every minute.

### Per-key ingest quota

The per-address limits above cannot stop a public SDK key from being used by
many addresses, and they count requests, while one ingest request can carry
500 rows. Impression and event ingestion therefore also has a quota per
**credential**: each SDK key separately (the legacy `SDK_KEY` variable counts
as one key), and each user ingesting with a session or personal access token.

| Budget | Variable | Default (per minute) | Charged |
|---|---|---|---|
| Requests | `RATE_LIMIT_INGEST_KEY_PER_MINUTE` | `20000` | one per ingest request, empty batches included |
| Rows | `RATE_LIMIT_INGEST_ROWS_PER_MINUTE` | `60000` | one per row, or one per started KiB of request body when that is larger |

Impressions and events draw on the same budgets. A key may spend a full
minute's budget at once and then refills evenly (1,000 rows a second with the
defaults), like the per-address limits. A request is charged only if both
budgets admit it; otherwise it gets `429 Too Many Requests` with `Retry-After`
(at most 61 seconds) and **none of its rows are stored**. The per-address
limit is checked first, then the key, the batch size and the body size, then
the quota, all before anything is written to PostgreSQL.

The quota state is shared through Redis, so every instance enforces one
budget per key: a script checks and charges both budgets atomically, using the
Redis clock. It keeps one small hash per active credential, under
`<REDIS_CHANNEL>:ingest-quota:`, that expires about a second after the
credential's budgets have refilled (so at most about 61 seconds after its last
request). **If Redis does not answer within 0.5 s**, each instance enforces
the same budgets on its own for 5 seconds before trying Redis again: with *N*
instances a key can then reach up to *N* times its budget. Flag evaluation and
streaming are unaffected either way, and
`checkgate_ingest_quota_local_decisions_total` counts the requests decided
this way.

The SDKs send analytics fire-and-forget: a refused batch is dropped, not
retried, so a quota never causes a retry storm or unbounded buffering in the
SDK (each SDK buffers at most 10,000 impressions and 10,000 events). Analytics
beyond the quota are lost; flags keep working.

**Sizing.** Each SDK instance sends at most one impressions request and one
events request per flush interval (10 s by default), plus one per 50 buffered
rows. The defaults admit, per key, about 1,600 sessions that each flush both
every 10 s, or 1,000 rows a second in any mix. They bound the *rate* of growth, not the size of a small disk: sustained, the
default row budget is 86 million rows a day per key. Set the row budget to
about two to three times your measured peak
(`rate(checkgate_analytics_rows_total{outcome="accepted"}[5m]) * 60`) and
watch `checkgate_ingest_quota_rejections_total`. A deployment that limits at a
gateway can set both variables to `0`.

### Analytics retention

Impressions and goal events older than `ANALYTICS_RETENTION_DAYS` (default
**90**) are deleted automatically. Age is measured from when the server
received the row (`received_at`), not from the time an SDK reported, so a
client clock cannot make a row expire early or live longer.

What is **kept** regardless of age: the impressions of a **running or
paused** experiment's flag and the events of its goal, in the experiment's
environment. They expire normally once the experiment is completed or deleted.
Nothing else is touched: flags, revisions, audit logs, segments, experiments,
webhook and integration deliveries and the rest keep their own lifetimes.

How cleanup runs:

- Every instance checks once a minute (or every
  `ANALYTICS_RETENTION_INTERVAL_SECS`, if shorter). Only one instance at a time
  can run a pass (a PostgreSQL advisory lock), and a pass runs only if the last
  completed one, on any instance, finished at least
  `ANALYTICS_RETENTION_INTERVAL_SECS` ago (default one hour).
- A pass deletes in batches of `ANALYTICS_RETENTION_BATCH_SIZE` rows (default
  5,000) in `received_at` order, using the existing `received_at` indexes. Each
  batch is a short transaction of its own, with a 30 s statement timeout, and
  the pass pauses 50 ms between batches. A running pass uses one connection
  of the instance's pool (`DB_MAX_CONNECTIONS`). Flag reads and writes never
  touch these tables, so ingestion, evaluation and streaming are not blocked.
- A failed batch rolls back whole; the next check (on any instance) resumes
  from the oldest remaining row. A pass interrupted by shutdown stops between
  batches and is resumed the same way.
- Startup deletes nothing: the first check comes a minute after the server
  starts (or after the interval, if shorter).

`ANALYTICS_RETENTION_DAYS=0` turns retention off; the server logs a warning
at startup that the tables grow without bound. Changing the value takes effect
at the next pass; lowering it deletes the newly expired rows in batches.

**Effect on analytics.** Every report reads only the rows that are kept:

| Report | Reads | With retention |
|---|---|---|
| Experiment results | all impressions of the flag and events of the goal | Complete while the experiment is running or paused. After it is completed, rows older than the retention expire and its results shrink, so record the results before completing an experiment that ran longer than the retention |
| Exposure (`/impressions/exposure`) | totals over all kept rows; daily timeline up to 90 days | Totals cover the retention period; with the default of 90 days the longest timeline is complete |
| Impression stats, impression list, event keys | all kept rows | Cover the retention period |

**Storage.** To see what the analytics tables hold:

```sql
SELECT relname,
       pg_size_pretty(pg_total_relation_size(oid)) AS total_size,
       reltuples::bigint AS approx_rows
FROM pg_class WHERE relname IN ('impressions', 'events');

-- Rows kept past the retention by running or paused experiments:
SELECT x.key, x.status, count(i.*) AS impressions
FROM experiments x JOIN impressions i
  ON i.environment_id = x.environment_id AND i.flag_key = x.flag_key
WHERE x.status IN ('running', 'paused')
  AND i.received_at < NOW() - INTERVAL '90 days'
GROUP BY x.key, x.status;
```

PostgreSQL reuses the space of deleted rows for new ones after `VACUUM`
(autovacuum does this); the files shrink only after `VACUUM FULL` or
`pg_repack`. With retention in steady state, the tables stay at about
*ingestion rate × retention period*. Use the metrics in
[Observability](./operations/observability.md#analytics-ingestion-and-retention)
to watch ingestion and cleanup.

### Behind a reverse proxy or load balancer

By default the client is the TCP peer, and `X-Forwarded-For` is **ignored** —
otherwise any client could choose its own rate-limit identity by sending the
header. If Checkgate runs behind a proxy, list the proxy's addresses so its
header is believed:

```bash
TRUSTED_PROXIES="10.0.0.0/8, 192.168.1.10"   # IPs and/or CIDR ranges
```

The `X-Forwarded-For` chain is read from the right: hops that are trusted
proxies are skipped and the first untrusted hop is the client. Entries a
client supplies itself are never used. If the header is malformed, the request
is attributed to the nearest trusted proxy. An invalid `TRUSTED_PROXIES` value
stops the server at startup. Only trust addresses that your proxies actually
use — a trusted range that untrusted clients can connect from lets them spoof
any address.

If your proxy sends the RFC 7239 `Forwarded` header instead, set
`TRUSTED_PROXY_HEADER=forwarded`; its `for=` parameters are read with the same
right-to-left rules, and `X-Forwarded-For` is then ignored (a proxy usually
passes the header it does not set through from the client). Obfuscated
identifiers (`for=_hidden`, `for=unknown`) attribute the request to the nearest
trusted proxy.

Rate limits are kept per instance: with *N* replicas behind a balancer that
spreads one client's requests, that client can make up to *N* times the
configured budget. Divide the limits by your replica count if you need a
cluster-wide figure. (The per-key ingest quota is shared by all instances
through Redis.)

## AWS

### EC2 Deployment

1. Launch an EC2 instance (t3.micro is sufficient for most workloads)
2. Install Docker:
   ```bash
   sudo yum update -y && sudo yum install -y docker
   sudo systemctl start docker
   ```
3. Either use the bundled services (all-in-one — just follow [Docker Compose](#docker-compose-all-in-one)),
   or set up RDS PostgreSQL and ElastiCache Redis and continue below.
4. Create a `.env` file:
   ```bash
   DATABASE_URL=postgres://user:pass@rds-endpoint:5432/checkgate
   REDIS_URL=redis://elasticache-endpoint:6379
   SESSION_SECRET=your-random-secret
   SETUP_TOKEN=your-random-setup-token
   COOKIE_SECURE=true
   ```
5. Run:
   ```bash
   docker run -d --env-file .env -p 3000:3000 \
     ghcr.io/checkgate-sh/checkgate:<version>
   ```

### ECS (Fargate)

A basic task definition (keep the secrets in Secrets Manager or SSM and
reference them with `secrets` in production):

```json
{
  "family": "checkgate",
  "networkMode": "awsvpc",
  "requiresCompatibilities": ["FARGATE"],
  "cpu": "256",
  "memory": "512",
  "containerDefinitions": [
    {
      "name": "checkgate",
      "image": "ghcr.io/checkgate-sh/checkgate:<version>",
      "portMappings": [{"containerPort": 3000}],
      "environment": [
        {"name": "DATABASE_URL", "value": "postgres://..."},
        {"name": "REDIS_URL", "value": "redis://..."},
        {"name": "SESSION_SECRET", "value": "your-random-secret"},
        {"name": "SETUP_TOKEN", "value": "your-random-setup-token"},
        {"name": "TRUSTED_PROXIES", "value": "10.0.0.0/16"},
        {"name": "COOKIE_SECURE", "value": "true"}
      ],
      "healthCheck": {
        "command": ["CMD-SHELL", "curl -f http://localhost:3000/health || exit 1"],
        "interval": 30, "timeout": 10, "retries": 3, "startPeriod": 30
      }
    }
  ]
}
```

Set `TRUSTED_PROXIES` to the subnets your load balancer connects from.

### Load Balancing

Checkgate keeps no flag state in server memory: SSE bootstrap, snapshot polling and the REST API read PostgreSQL, and live changes fan out to every instance through Redis pub/sub. Run as many instances as you need behind an ALB. (The SDK health page is the exception — each instance lists only its own SSE connections.)

No sticky sessions are needed: every instance relays every change to its own
SSE clients, and a reconnecting client may land on any instance. Keep the idle
timeout above 30 s (heartbeats are sent after 15 s of silence, `SSE_HEARTBEAT_INTERVAL_SECS`), health-check
`/health/ready`, and set the deregistration delay to about
`SHUTDOWN_DRAIN_DELAY_SECS`. See
[Deployment, failure and recovery](./operations/failure-recovery.md#load-balancers).

## Health Checks

| Endpoint | Use for | Fails when |
|---|---|---|
| `GET /health/live` | Liveness | Never while the process serves requests — a dependency outage must not cause restarts |
| `GET /health/ready` | Readiness / load-balancer health | Draining for shutdown, or PostgreSQL unreachable (and Redis down, if `READY_REQUIRES_REDIS=true`) |
| `GET /health` | Unchanged legacy check (`OK`) | Same as liveness |

None of them is rate limited. `/health/ready` returns
`{"status": "ready" | "degraded" | "unavailable" | "draining", "checks": {...}}`;
`degraded` (Redis down, still serving) answers 200.

The image declares a `HEALTHCHECK` on `/health/live` (every 30 s), so
`docker ps` shows the container as `healthy`. On Kubernetes:

```yaml
terminationGracePeriodSeconds: 30
readinessProbe:
  httpGet: { path: /health/ready, port: 3000 }
  periodSeconds: 5
livenessProbe:
  httpGet: { path: /health/live, port: 3000 }
  periodSeconds: 10
```

with `SHUTDOWN_DRAIN_DELAY_SECS=5` and `SHUTDOWN_GRACE_PERIOD_SECS=25`. The
full probe, shutdown and capacity guidance is in
[Deployment, failure and recovery](./operations/failure-recovery.md); metrics
and logs in [Observability](./operations/observability.md).

## Upgrading

Checkgate uses [sqlx migrations](https://docs.rs/sqlx/latest/sqlx/macro.migrate.html) that run automatically on startup. If you pinned a version, update the image tag in `compose.yml` to the desired release first. From your deployment directory, run:

```bash
docker compose pull checkgate
docker compose up -d checkgate
```

The server applies any new migrations before accepting traffic. Existing data is preserved,
except as the release notes say: from 0.1.31, impressions and events older than
`ANALYTICS_RETENTION_DAYS` (default 90) are deleted after the upgrade — set it
before upgrading if you need them (see [Upgrading to 0.1.31](./operations/upgrading.md)).

### Upgrading to the release with deployment modes and setup tokens

- **You ran the image with `DATABASE_URL`/`REDIS_URL` set.** Earlier images
  ignored both and always used the bundled PostgreSQL inside the container, so
  your data is there — in the `/var/lib/postgresql/data` volume, if you mounted
  one. The new image connects to the URLs you pass, which will look like a fresh
  install. Before upgrading, dump the bundled database
  (`docker exec <container> pg_dump -U checkgate checkgate > checkgate.sql`)
  and restore it into your external database — or, to keep using the bundled
  database, remove `DATABASE_URL` and `REDIS_URL` from the container's
  environment.
- **Setup tokens.** Installations that already completed setup are unaffected.
  A never-set-up installation now needs the setup token
  ([First-run setup](#first-run-setup)).
- **Reverse proxies.** `X-Forwarded-For` is now honoured only from
  `TRUSTED_PROXIES`. Behind a proxy, set it — otherwise all clients share the
  proxy's rate-limit budget and are logged under its address.
- **Rate limits.** The single 60 requests/minute limit is replaced by the
  per-class budgets in [Rate limits](#rate-limits), which are higher for every
  class except login/setup (20/minute).
- **Missing connection settings.** The bare binary (outside the image) no longer
  defaults `DATABASE_URL`/`REDIS_URL` to `localhost` in release builds.

## Backup

Back up your PostgreSQL database using standard tooling:

```bash
pg_dump -h localhost -U checkgate checkgate > checkgate-backup.sql
```

Redis is used only for pub/sub message passing and does not need to be backed up — no durable state is stored there.
