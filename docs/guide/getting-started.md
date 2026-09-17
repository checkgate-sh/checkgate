---
title: "Getting Started with Checkgate — Feature Flags in Minutes"
description: "Step-by-step guide to running Checkgate locally, completing the setup wizard, creating your first flag, and evaluating it in-process — no network call on every isEnabled() — using the Node.js SDK."
---

# Getting Started

This guide walks you through running Checkgate locally and evaluating your first feature flag.

## Prerequisites

- Docker and Docker Compose
- Node.js 18+ (for the Node.js SDK example)

## 1. Start the Server

Use the published all-in-one Docker image. No repository clone or local build is needed.

Create a directory for your deployment:

```bash
mkdir checkgate
cd checkgate
```

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

Pull the release image and start the container:

```bash
docker compose pull
docker compose up -d
```

This starts a single container with PostgreSQL, Redis, the Checkgate server, and the React dashboard all bundled together. The dashboard is available at [http://localhost:3000](http://localhost:3000). PostgreSQL data is stored in a named Docker volume and survives container replacement.

For a fixed release, replace `:latest` in `compose.yml` with a published version tag such as `:<version>` (without the leading `v`).

## 2. Complete the Setup Wizard

On first run, you are redirected to the setup wizard at [http://localhost:3000/setup](http://localhost:3000/setup).

The wizard collects:
1. **Workspace name** — your company or team name (shown on the login page)
2. **First project name** — the initial project (e.g. "My App"); more projects can be added later
3. **Your name** and **work email** — used for the first admin account
4. **Password** — minimum 8 characters
5. **SDK key** — auto-generated for the Production environment; copy it now for use in your applications

After completing the wizard you are logged in as the first admin.

## 3. Create Your First Flag

Open the dashboard and select the **Development** environment from the sidebar switcher. Navigate to **Feature Flags** and create a flag:

- **Key**: `new-checkout-flow`
- **Enabled**: true
- **Rollout**: 50%

Or use the REST API directly with your SDK key:

```bash
SDK_KEY=sk_live_your_key_here
ENV_ID=your-environment-uuid  # copy from the dashboard URL or list environments

curl -X POST "http://localhost:3000/api/environments/${ENV_ID}/flags" \
  -H "Authorization: Bearer ${SDK_KEY}" \
  -H "Content-Type: application/json" \
  -H "X-Checkgate-Request: true" \
  -d '{
    "key": "new-checkout-flow",
    "is_enabled": true,
    "rollout_percentage": 50,
    "description": "New checkout UI rollout",
    "rules": []
  }'
```

To get the list of projects and their IDs, then the environments within a project:

```bash
# List projects (returns project IDs)
curl http://localhost:3000/api/projects \
  -H "Authorization: Bearer ${SDK_KEY}"

# List environments for a project
PROJECT_ID=your-project-uuid
curl "http://localhost:3000/api/projects/${PROJECT_ID}/environments" \
  -H "Authorization: Bearer ${SDK_KEY}"
```

## 4. Install the SDK

::: code-group

```bash [Node.js]
npm install @checkgate/node
```

```bash [Web]
npm install @checkgate/web
```

```bash [React Native]
npm install @checkgate/react-native
```

:::

## 5. Connect and Evaluate

```typescript
import { CheckgateClient } from '@checkgate/node'

const client = new CheckgateClient({
  serverUrl: 'http://localhost:3000',
  sdkKey: 'sk_live_your_key_here',
})

await client.connect()

// Evaluate a flag for a user
const enabled = client.isEnabled('new-checkout-flow', 'user-123', {
  email: 'alice@example.com',
  plan: 'pro',
})

console.log('New checkout:', enabled)
```

The client connects once via SSE, downloads all flags, and evaluates locally on every `isEnabled()` call. No network round-trips at evaluation time.

## 6. Production Setup

For production, set a strong `SESSION_SECRET` environment variable so session cookies are cryptographically secure. Generate one with:

```bash
openssl rand -hex 32
```

Create a `.env` file alongside `compose.yml`:

```dotenv
SESSION_SECRET=your-64-char-secret
POSTGRES_PASSWORD=strong-db-password
COOKIE_SECURE=true
```

Set `COOKIE_SECURE=true` when serving Checkgate over HTTPS; keep it `false` for local HTTP. Choose your database password before the first startup; changing the variable does not reset an existing database password.

Then apply the configuration:

```bash
docker compose up -d
```

SDK keys are managed per-project from **Project Settings → SDK Keys**. Each key is bound to a specific environment — the key implicitly tells the server which project and environment to serve flags from. Additional keys can be created and revoked there without restarting the server.

See [Self-Hosting](/self-hosting) for full production deployment options including AWS, environment variable reference, and upgrade instructions.
