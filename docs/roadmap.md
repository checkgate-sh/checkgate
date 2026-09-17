# Checkgate Product Roadmap

This document outlines the vision and future development priorities for Checkgate. Items are ordered by **impact** — what unblocks the most users, most quickly.

---

## 🟣 Enterprise Governance & Scale (In Progress)
*Goal: Enable large teams to move fast without breaking things.*

- [ ] **SSO / SAML / SCIM**: Enterprise identity provider integration for login and user provisioning. Parked for now — needs a real IdP to integrate/verify against (Okta, Azure AD, etc.), unlike the other Phase 5 items above which were self-contained and testable end-to-end against this repo alone.
- [ ] **VS Code Extension**: Inline flag status, targeting rules, and direct links to the dashboard from your editor.

---

## 🟠 SDK & Ecosystem Expansion
*Goal: Meet developers in every language and runtime they already use.*

Every new SDK wraps the **same shared Rust evaluation core** in [`core/`](https://github.com/checkgate-sh/checkgate/tree/main/core) — exactly how the existing Node.js (NAPI), Web (WASM), React Native (JSI), and Flutter (FFI) SDKs do — so local, sub-microsecond evaluation and flag semantics (rules, rollouts, segments, prerequisites, weighted variants) stay **identical across every language**. Each also inherits the shared resilience layer: SSE streaming with backoff, offline persistence, and the `/flags/snapshot` poll fallback.

**Server-side languages** (backend/local evaluation)
- [ ] **Python SDK**: PyO3 bindings to the core — Django / FastAPI / Flask apps and data/ML services.
- [ ] **Go SDK**: A cgo-FFI **local-evaluation** SDK, complementing the existing [`checkgate-go`](https://github.com/checkgate-sh/checkgate/tree/main/integrations/checkgate-go) *management* client that already backs the Terraform provider and Kubernetes operator.
- [ ] **Java / Kotlin (JVM) SDK**: JNI bindings — Spring Boot and other JVM backends.
- [ ] **Ruby SDK**: Rails apps and Sidekiq workers.
- [ ] **PHP SDK**: Laravel / Symfony.
- [ ] **.NET / C# SDK**: ASP.NET Core.
- [ ] **Rust-native SDK**: Depend on `checkgate-core` directly, no FFI layer.

**Mobile & desktop**
- [ ] **Swift SDK**: C-FFI over the core, distributed via Swift Package Manager — native iOS / macOS / visionOS.
- [ ] **Kotlin Multiplatform / Android SDK**: JNI over the core, sharing logic with the JVM SDK for Android apps.

**Standards & interop**
- [ ] **OpenFeature providers**: Official [OpenFeature](https://openfeature.dev) provider(s) so Checkgate drops into any OpenFeature-instrumented app. High leverage — implement one spec, plug into many language ecosystems at once.

---

## 🔴 Other High-Impact Bets (Ideas)
*Bigger swings worth validating with users before committing.*

- [ ] **Flag Relay / Edge CDN**: A lightweight read-through relay so very large fleets fan out from a regional cache instead of every SDK instance streaming from origin — horizontal scale for millions of connections.
- [ ] **OpenTelemetry export**: Emit evaluation, impression, and experiment metrics/traces to any OTel backend (Grafana, Datadog, Honeycomb).
- [ ] **Warehouse sync**: Stream impressions, goal events, and experiment results to BigQuery / Snowflake / Postgres for BI and data-science workflows.
- [ ] **Slack / Teams approvals & alerts**: Review and approve change requests, and receive flag-change notifications, where teams already work.
  - [x] **Alerts** — flag and change-request activity delivered to Slack (Block Kit) and Teams (MessageCard) via incoming webhooks, configured per environment with per-event filtering.
  - [ ] **Approvals** — approve/reject a change request from the message itself. Needs a signed public interactions endpoint and a Slack↔Checkgate identity mapping to keep the "no self-approval" rule enforceable.

---

> [!TIP]
> **Want to contribute?** I welcome ideas and pull requests! Check the [Contributing Guide](https://github.com/checkgate-sh/checkgate/blob/main/CONTRIBUTING.md) to get started.
