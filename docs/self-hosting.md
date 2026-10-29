---
title: "Self-Hosting Checkgate — Docker & AWS Deployment Guide"
description: "Deploy Checkgate on your own infrastructure using Docker Compose, AWS EC2, or ECS Fargate. Your flag data stays on your servers. Includes environment variable reference, load balancing tips, and upgrade instructions."
---

# Self-Hosting

Checkgate is designed to be self-hosted. The all-in-one Docker image bundles PostgreSQL, Redis, the server, and the dashboard into a single container — no external services required.

## Docker Compose (Recommended)

Run the published release image with Docker Compose. No repository clone or local build is needed.

Create a deployment directory and save the following as `compose.yml`:

```yaml
services:
  checkgate:
    image: ghcr.io/checkgate-sh/checkgate:latest
    container_name: checkgate
    ports:
      - "3000:3000"
    environment:
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:-checkgate}
      SESSION_SECRET: ${SESSION_SECRET:-}
      COOKIE_SECURE: ${COOKIE_SECURE:-false}
    volumes:
      - checkgate_pgdata:/var/lib/postgresql/data
      - checkgate_redis:/var/lib/redis
    restart: unless-stopped

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
- **PostgreSQL** — persistent flag and user storage
- **Redis** — pub/sub for real-time multi-instance sync
- **Checkgate server** — REST API + SSE stream on port `3000`
- **React dashboard** — served at `http://localhost:3000`

### Configuration

Create a `.env` file in the deployment directory, alongside `compose.yml`:

```bash
SESSION_SECRET=your-random-64-char-secret   # Required in production
POSTGRES_PASSWORD=strong-db-password        # Replaces default "checkgate"
COOKIE_SECURE=true                          # Set false only for local HTTP development
```

Then run:

```bash
docker compose up -d
```

## Docker Images

One image is published per release:

| Image | Description |
|-------|-------------|
| `ghcr.io/checkgate-sh/checkgate:latest` | All-in-one: PostgreSQL + Redis + server + dashboard |
| `ghcr.io/checkgate-sh/checkgate:<version>` | Pinned release version (use the version tag without its leading `v`) |

The image is multi-architecture: `linux/amd64` and `linux/arm64`. Docker selects the matching architecture automatically. For production, replace `:latest` in `compose.yml` with a published version tag to control upgrades.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `SESSION_SECRET` | _(insecure dev key)_ | Secret for encrypting session cookies. Use a random ≥32-char string in production. |
| `DATABASE_URL` | `postgres://checkgate:password@localhost/checkgate` | PostgreSQL connection string (used when not using bundled PG) |
| `REDIS_URL` | `redis://localhost:6379` | Redis connection string (used when not using bundled Redis) |
| `POSTGRES_PASSWORD` | `checkgate` | Password for the bundled PostgreSQL instance |
| `POSTGRES_USER` | `checkgate` | Username for the bundled PostgreSQL instance |
| `POSTGRES_DB` | `checkgate` | Database name for the bundled PostgreSQL instance |
| `COOKIE_SECURE` | `true` | Set `false` for local HTTP development. Controls the `Secure` cookie flag. |
| `SDK_KEY` | _(unset)_ | Legacy env var: adds an extra accepted SDK key without a DB entry |
| `PUBLIC_DIR` | `public` | Directory to serve dashboard static files from |
| `PORT` | `3000` | Port the server listens on |
| `DB_MAX_CONNECTIONS` | `10` | PostgreSQL connection pool size |
| `RUST_LOG` | `info` | Log level (`error`, `warn`, `info`, `debug`, `trace`) |

### Security

Always set `SESSION_SECRET` in production. Without it, session cookies use a hardcoded dev key that is public knowledge — anyone can forge sessions.

```bash
# Generate a secure session secret
openssl rand -hex 32
```

SDK keys are managed through the dashboard Settings page. The first key is auto-generated on first boot. Additional keys can be created and revoked without restarting the server.

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
password-guessing run cannot exhaust the SDK budget. `/health` is never
limited. Opening an SSE stream counts once; events delivered on an open stream
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
Limiter state holds only recently active clients (IPv6 clients are grouped by
/64) and is pruned every minute.

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

## External PostgreSQL and Redis

To use your own PostgreSQL and Redis instead of the bundled ones, run the image directly and pass the connection strings:

```bash
docker run -d -p 3000:3000 \
  -e DATABASE_URL="postgres://user:password@your-postgres:5432/checkgate" \
  -e REDIS_URL="redis://your-redis:6379" \
  -e SESSION_SECRET="your-secret" \
  ghcr.io/checkgate-sh/checkgate:latest
```

## AWS

### EC2 Deployment

1. Launch an EC2 instance (t3.micro is sufficient for most workloads)
2. Install Docker:
   ```bash
   sudo yum update -y && sudo yum install -y docker
   sudo systemctl start docker
   ```
3. Set up RDS PostgreSQL and ElastiCache Redis (optional — use the bundled versions for simpler setups)
4. Create a `.env` file:
   ```bash
   DATABASE_URL=postgres://user:pass@rds-endpoint:5432/checkgate
   REDIS_URL=redis://elasticache-endpoint:6379
   SESSION_SECRET=your-random-secret
   COOKIE_SECURE=true
   ```
5. Run:
   ```bash
   docker run -d --env-file .env -p 3000:3000 \
     ghcr.io/checkgate-sh/checkgate:latest
   ```

### ECS (Fargate)

A basic task definition:

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
      "image": "ghcr.io/checkgate-sh/checkgate:latest",
      "portMappings": [{"containerPort": 3000}],
      "environment": [
        {"name": "DATABASE_URL", "value": "postgres://..."},
        {"name": "REDIS_URL", "value": "redis://..."},
        {"name": "SESSION_SECRET", "value": "your-random-secret"},
        {"name": "COOKIE_SECURE", "value": "true"}
      ]
    }
  ]
}
```

### Load Balancing

Checkgate keeps no flag state in server memory: SSE bootstrap, snapshot polling and the REST API read PostgreSQL, and live changes fan out to every instance through Redis pub/sub. Run as many instances as you need behind an ALB. (The SDK health page is the exception — each instance lists only its own SSE connections.)

**Important**: The SSE stream (`/stream`) requires sticky sessions or a long-lived HTTP/2 connection. Configure your ALB target group with:
- **Protocol**: HTTP/1.1 (SSE is not compatible with HTTP/2 connection multiplexing on ALB)
- **Stickiness**: Enabled (duration-based, any duration)

Or rely on **Redis pub/sub fan-out** (built-in): all instances subscribe to Redis and will forward updates to their connected SDK clients regardless of which instance received the API write.

## Health Checks

The server exposes a health check endpoint:

```http
GET /health
```

Returns `200 OK` with body `OK`. Use this for load balancer and container health checks (as configured in the Dockerfile's `HEALTHCHECK` instruction).

## Upgrading

Checkgate uses [sqlx migrations](https://docs.rs/sqlx/latest/sqlx/macro.migrate.html) that run automatically on startup. If you pinned a version, update the image tag in `compose.yml` to the desired release first. From your deployment directory, run:

```bash
docker compose pull checkgate
docker compose up -d checkgate
```

The server applies any new migrations before accepting traffic. Existing data is preserved.

## Backup

Back up your PostgreSQL database using standard tooling:

```bash
pg_dump -h localhost -U checkgate checkgate > checkgate-backup.sql
```

Redis is used only for pub/sub message passing and does not need to be backed up — no durable state is stored there.
