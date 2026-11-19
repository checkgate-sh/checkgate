<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/banner-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="assets/banner-light.svg">
    <img src="assets/banner-dark.svg" alt="Checkgate — feature flags without the round-trip" width="100%">
  </picture>
</p>

# Checkgate — Feature Flags Without the Round-Trip.

[![CI](https://github.com/checkgate-sh/checkgate/actions/workflows/ci.yml/badge.svg)](https://github.com/checkgate-sh/checkgate/actions/workflows/ci.yml)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![npm @checkgate/node](https://img.shields.io/npm/v/@checkgate/node?label=npm%20node)](https://www.npmjs.com/package/@checkgate/node)
[![npm @checkgate/web](https://img.shields.io/npm/v/@checkgate/web?label=npm%20web)](https://www.npmjs.com/package/@checkgate/web)
[![npm @checkgate/react-native](https://img.shields.io/npm/v/@checkgate/react-native?label=npm%20react-native)](https://www.npmjs.com/package/@checkgate/react-native)

**Checkgate is an open-source feature flag engine that evaluates flags locally — no network call, no latency, no SaaS vendor.** Every flag decision happens inside your process, while a persistent SSE stream keeps every SDK instance in sync, typically within tens of milliseconds of a change.

It is proudly built in Rust and ships with native SDKs for Node.js (NAPI), browsers (WebAssembly), React Native (JSI), and Flutter (FFI). In the v0.1.31 benchmark, a change reached 1,000 connected SDK clients with a p50 of 39 ms, and none of 1.2 million deliveries was missed.

**[Explore the Documentation →](https://checkgate-sh.github.io/checkgate)**

---

## Overview

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/overview-dark.svg">
    <img src="assets/overview-light.svg" width="820" alt="A flag change flows from the dashboard to the Checkgate server, which pushes it over SSE to every SDK, where flags evaluate locally in-process.">
  </picture>
</p>

<p align="center">
  <em>A flag change is written once, then pushed to every connected SDK in milliseconds — after which each <code>isEnabled()</code> is a local, in-process lookup.</em>
</p>

---

## Quick start

Run the all-in-one image (PostgreSQL, Redis, the server and the dashboard in one container). Save as `compose.yml`:

```yaml
services:
  checkgate:
    image: ghcr.io/checkgate-sh/checkgate:latest
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

```bash
docker compose up -d   # then finish the setup wizard at http://localhost:3000/setup
npm install @checkgate/node
```

```typescript
import { CheckgateClient } from '@checkgate/node'

const client = new CheckgateClient({ serverUrl: 'http://localhost:3000', sdkKey: 'sk_live_your_key_here' })
await client.connect()

// Evaluated in-process: no network call per decision.
const enabled = client.isEnabled('new-checkout-flow', 'user-123', { plan: 'pro' })
```

The full walkthrough (first flag, SDK keys, production settings): [Getting Started](docs/guide/getting-started.md).

---

## Features

- **In-process evaluation** — flags are evaluated in local memory: well under a microsecond in the Rust core, 0.8–4 µs through the Node.js SDK
- **Real-time updates** — SSE push, not polling; changes typically land in tens of milliseconds
- **Targeting rules** — match by any user attribute (`email`, `plan`, `region`, …)
- **Percentage rollouts** — deterministic MurmurHash3 bucketing; sticky and independent per flag
- **Rust evaluation core** — compiled to NAPI, WASM, JSI, or FFI depending on platform
- **Self-hosted** — single binary + PostgreSQL + Redis; your data never leaves your infra

---

## Architecture & Performance

*Local evaluation. Real-time updates. Lightweight infrastructure.*

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/benchmarks/performance-dark.svg">
    <img src="assets/benchmarks/performance-light.svg" width="100%" alt="Checkgate v0.1.31 architecture and performance, measured on a 4-core laptop with the server limited to 1 vCPU and 512 MiB, each figure next to v0.1.30 measured in the same session: Node SDK isEnabled() 1.0 µs (v0.1.30: 7.2 µs); 591 req/s snapshot throughput at 1,000 concurrent clients (v0.1.30: 489); 1.51 ms server CPU per request (v0.1.30: 1.92 ms), peak RSS 59 MiB; update propagation p99 198 ms at 1,000 SDK clients pooled over 6 runs, median run 84 ms (v0.1.30: 75 ms), none of 1.2 million deliveries missed.">
  </picture>
</p>

Measured on v0.1.31, and on v0.1.30 again in the same session with runs alternated, on a 4-core Intel laptop in desktop use. The server ran in Docker with a 1 vCPU / 512 MiB limit; 3 runs per measurement, 6 for propagation. The higher pooled p99 comes from two runs made while the host was still loaded; the reasons are in the methodology.
Methodology, every run and the raw data: [Public benchmarks](docs/performance/benchmarks.md).

---

## Infrastructure Cost & Savings

*Understand the infrastructure required to run your own feature flags.*

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/benchmarks/cost-dark.svg">
    <img src="assets/benchmarks/cost-light.svg" width="100%" alt="Checkgate v0.1.31 infrastructure cost, measured on one server limited to 1 vCPU and 512 MiB with PostgreSQL and Redis: 10,000 streaming clients held at 14.7 % of one core and 448 MiB idle, with 6.5 KB of heartbeats per client per hour. For 1,000 clients, streaming needs 8.7 GB of traffic a month (calculated) and delivers a change in 33 ms (p50); polling every 60, 30 or 10 seconds needs 669, 1,340 or 4,020 GB and leaves flags 30, 15 or 5 seconds stale (p50). Projected at AWS us-east-1 list prices accessed 2026-11-19: $38.56 a month for 100 to 10,000 streaming clients, and $89.80, $150.08 or $390.94 for 1,000 clients polling. A projection, not a quote; polling here returns the whole flag set every time.">
  </picture>
</p>

What one server spends to keep 100 to 10,000 clients up to date over SSE, the same clients polling every 10, 30 or 60 s instead, and that infrastructure priced at dated AWS list prices. Measured, calculated and projected figures are kept apart, and PostgreSQL and Redis are included.
Method, assumptions and sources: [Infrastructure cost and efficiency](docs/performance/cost-efficiency.md).

---

## Screenshots

*Indigo dashboard with the Checkgate gate logo, shown with example data for Vantage Robotics and administrator Juan Dela Cruz. [Explore the dashboard guide](docs/guide/dashboard.md).*

**Dashboard overview** — flag counts, rollout status, and recent activity at a glance.

![Dashboard overview](assets/screenshots/01-dashboard.png)

**Feature flags** — tags, types, rollout percentage, and one-click enable/disable per environment.

![Feature flags list](assets/screenshots/02-feature-flags.png)

**Flag editor** — targeting rules, tags, ownership, and prerequisite (dependent) flags.

![Flag editor](assets/screenshots/03-flag-editor.png)

**Change requests** — require a second reviewer before a flag change takes effect in sensitive environments; self-approval is blocked.

![Change requests](assets/screenshots/04-change-requests.png)

**Cross-environment diff** — see what's different between environments before promoting, with a one-click sync.

![Compare environments](assets/screenshots/05-environment-diff.png)

**Personal access tokens** — scoped, revocable API credentials for CI/CD and Terraform, as an alternative to admin-equivalent SDK keys.

![Personal access tokens](assets/screenshots/06-settings-tokens.png)

**Environments** — isolate configuration across Production, Staging, UAT, and Development, with per-environment approval gates.

![Environments](assets/screenshots/07-environments.png)

**Collapsible sidebar** — reclaim screen space on data-dense pages; the whole dashboard is full-width by default.

![Collapsed sidebar](assets/screenshots/08-sidebar-collapsed.png)

---

## Documentation

| Topic | Link |
|-------|------|
| Why Checkgate / comparisons | [What is Checkgate?](docs/guide/what-is-checkgate.md) |
| System architecture | [Architecture](docs/guide/architecture.md) |
| Flags, rules, rollout concepts | [Core Concepts](docs/guide/concepts.md) |
| Dashboard tour | [Dashboard](docs/guide/dashboard.md) |
| Step-by-step setup | [Getting Started](docs/guide/getting-started.md) |
| REST API + SSE stream reference | [API Reference](docs/api-reference.md) |
| Node.js SDK | [SDK: Node.js](docs/sdks/nodejs.md) |
| Web (WASM) SDK | [SDK: Web](docs/sdks/web.md) |
| React Native (JSI) SDK | [SDK: React Native](docs/sdks/react-native.md) |
| Flutter (FFI) SDK | [SDK: Flutter](docs/sdks/flutter.md) |
| Type-safe codegen CLI | [`@checkgate/cli`](cli/README.md) |
| Edge evaluation (Workers, Fly.io) | [`@checkgate/edge`](edge/README.md) |
| SSR / bootstrap (Next.js, Remix, …) | [`@checkgate/ssr`](ssr/README.md) |
| Terraform / OpenTofu provider | [terraform-provider-checkgate](integrations/terraform-provider-checkgate/README.md) |
| Kubernetes operator | [checkgate-operator](integrations/kubernetes-operator/README.md) |
| Docker, AWS, env vars | [Self-Hosting](docs/self-hosting.md) |
| Enterprise Setup & Migration | [Enterprise Setup & Migration](docs/enterprise-setup.md) |

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Apache License 2.0 — © 2025


