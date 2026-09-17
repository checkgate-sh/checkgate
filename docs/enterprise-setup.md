---
title: "Enterprise Setup — Migrating from LaunchDarkly to Checkgate"
description: "A production-ready guide for engineering teams migrating from LaunchDarkly to self-hosted Checkgate. Covers infrastructure provisioning, AWS deployment, dashboard onboarding, and SDK integration."
---

# Enterprise Setup Guide: Migrating from LaunchDarkly

This guide is designed for engineering teams looking to transition from a managed service like LaunchDarkly to a self-hosted Checkgate environment. It focuses on high-performance, data-sovereign feature flagging.

## Phase 1: Infrastructure Provisioning

Checkgate is designed to be "stateless" and "self-healing," meaning it can be scaled horizontally behind a load balancer as long as all instances share the same PostgreSQL and Redis backends.

### 1. The Database (PostgreSQL)
Checkgate uses PostgreSQL for long-term persistence of flags, user roles, and audit logs.
- **Requirement**: A standard PostgreSQL instance (RDS, Cloud SQL, or self-hosted).
- **Automation**: Checkgate handles its own schema management. When you provide a `DATABASE_URL`, the server automatically runs migrations on startup, creating and updating tables as needed. No manual SQL scripts are required.

### 2. The Real-Time Sync (Redis)
Checkgate uses Redis for inter-instance communication and real-time updates.
- **Requirement**: A standard Redis instance (ElastiCache, MemoryStore, or self-hosted).
- **The Sync Engine**: Checkgate uses the **Redis Pub/Sub** pattern. When a flag is updated on one server instance, it broadcasts a message via Redis. All other instances receive the update and refresh their local memory caches instantly.
- **Resilience**: If Redis is temporarily unavailable, Checkgate continues to serve flags from its in-memory store; real-time updates will resume as soon as the connection is restored.

---

## Phase 2: Production Deployment

For production environments, we recommend deploying the **Checkgate Docker image** to an orchestrator like AWS ECS (Fargate), AWS EC2, or Kubernetes.

### Example: AWS ECS / EC2 Start
```bash
docker run -d -p 3000:3000 \
  -e DATABASE_URL="postgres://user:pass@your-rds-endpoint:5432/checkgate" \
  -e REDIS_URL="redis://your-elasticache-endpoint:6379" \
  -e SESSION_SECRET="$(openssl rand -hex 32)" \
  -e COOKIE_SECURE="true" \
  ghcr.io/checkgate-sh/checkgate:latest
```

---

## Phase 3: The Dashboard Onboarding

Once your Control Plane is accessible (e.g., at `https://flags.internal.com`), you will be redirected to the setup wizard at `/setup` on first visit:

1. **Workspace name** — your company or team name, shown on the login page.
2. **Admin account** — your name, work email, and a password (minimum 8 characters).
3. **SDK key** — auto-generated on first boot; copy it now for use in your applications.

After completing the wizard you are logged in as the first admin. Additional users can be invited from the **Users** page, and additional SDK keys can be created from **Settings**.

---

## Phase 4: App Integration (SDKs)

Transitioning from LaunchDarkly involves replacing the LD client with a Checkgate SDK.

### example: Flutter / Dart Integration
```dart
// 1. Initialize the client
final checkgate = CheckgateClient(
  url: "https://flags.internal.com",
  sdkKey: "sk_live_..."
);

await checkgate.initialize();

// 2. Evaluate flags locally (zero latency)
bool showNewFeature = checkgate.isEnabled("new_feature_v2");
```

## Why Enterprise Teams Choose Checkgate
- **Data Sovereignty**: user identities and PII never leave your network.
- **Zero-Latency Evaluation**: Decisions happen in microseconds inside your application's memory using Rust-powered FFI.
- **Cost Transparency**: No per-user, per-seat, or per-flag pricing. Scale to millions of users at the cost of a small cloud instance.
