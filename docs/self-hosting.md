---
title: "Self-Hosting Checkgate — Docker & AWS Deployment Guide"
description: "Deploy Checkgate on your own infrastructure using Docker Compose, AWS EC2, or ECS Fargate. Your flag data stays on your servers. Includes environment variable reference, load balancing tips, and upgrade instructions."
---

# Self-Hosting

Checkgate is designed to be self-hosted. The all-in-one Docker image bundles PostgreSQL, Redis, the server, and the dashboard into a single container — no external services required.

## Docker Compose (Recommended)

The simplest way to run Checkgate is with the provided `docker-compose.yml`:

```bash
docker compose up -d
```

This starts a single container that includes:
- **PostgreSQL** — persistent flag and user storage
- **Redis** — pub/sub for real-time multi-instance sync
- **Checkgate server** — REST API + SSE stream on port `3000`
- **React dashboard** — served at `http://localhost:3000`

### Configuration

Create a `.env` file in the project root:

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
| `ghcr.io/checkgate-sh/checkgate:<version>` | Pinned version (e.g. `1.2.0`) |

The image is multi-architecture: `linux/amd64` and `linux/arm64`.

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

Checkgate is stateless — the in-memory flag store is rebuilt from PostgreSQL on startup and kept in sync via Redis pub/sub. Run as many instances as you need behind an ALB.

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

Checkgate uses [sqlx migrations](https://docs.rs/sqlx/latest/sqlx/macro.migrate.html) that run automatically on startup. To upgrade:

```bash
docker pull ghcr.io/checkgate-sh/checkgate:latest
docker compose up -d --force-recreate
```

The server applies any new migrations before accepting traffic. Existing data is preserved.

## Backup

Back up your PostgreSQL database using standard tooling:

```bash
pg_dump -h localhost -U checkgate checkgate > checkgate-backup.sql
```

Redis is used only for pub/sub message passing and does not need to be backed up — no durable state is stored there.
