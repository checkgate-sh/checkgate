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

**Checkgate is an open-source feature flag engine that evaluates flags locally — no network call, no latency, no SaaS vendor.** Every flag decision happens inside your process in sub-microseconds, while a persistent SSE stream keeps every SDK instance in sync within 50 ms of a change.

It is proudly built in Rust and ships with native SDKs for Node.js (NAPI), browsers (WebAssembly), React Native (JSI), and Flutter (FFI). A persistent SSE stream propagates flag changes to every connected SDK instance in under 50 ms.

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

## Features

- **Sub-microsecond evaluation** — flags are evaluated entirely in local memory
- **Real-time updates** — SSE push, not polling; changes land in < 50 ms
- **Targeting rules** — match by any user attribute (`email`, `plan`, `region`, …)
- **Percentage rollouts** — deterministic MurmurHash3 bucketing; sticky and independent per flag
- **Rust evaluation core** — compiled to NAPI, WASM, JSI, or FFI depending on platform
- **Self-hosted** — single binary + PostgreSQL + Redis; your data never leaves your infra

---

## Screenshots

*Dashboard shown with example data for a fictional company, Vantage Robotics.*

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

Apache License 2.0 — © 2026 ThinkGrid Labs


