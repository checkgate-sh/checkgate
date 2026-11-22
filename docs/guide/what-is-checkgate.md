---
title: "What is Checkgate? — Feature Flags Without the Round-Trip"
description: "Learn how Checkgate eliminates per-evaluation network latency by running the flag decision in-process, and how local evaluation differs from remote evaluation."
---

# What is Checkgate?

Checkgate is a **self-hosted, open-source feature flag engine** built for teams that need fast, reliable flag evaluation without sending data to a third-party SaaS vendor.

## The Problem

When flags are evaluated by a remote service, every `isEnabled()` call is a network request. This introduces:

- **Latency** — each flag check waits for a network round trip
- **Reliability risk** — if the flag service is down, your flag evaluation breaks
- **Vendor lock-in** — migrating away is painful and expensive
- **Data privacy concerns** — user attributes are sent to an external service

## The Checkgate Approach

Checkgate takes a different approach: **local evaluation**.

1. Your server runs the Checkgate control plane (a single Rust binary)
2. Each SDK connects once via SSE and receives the full flag set
3. All evaluation happens **in-process, in memory** — no network calls
4. When flags change, the server pushes deltas via SSE, typically within tens of milliseconds

The result: **in-process flag evaluation** (84–456 ns median in the Rust core, 0.8–4 µs through the Node.js SDK, in the [v0.1.31 benchmarks](/performance/benchmarks)) with real-time updates and no network dependency at evaluation time.

## What You Get

Checkgate is a complete feature-flag platform, not just a toggle store:

- **Multivariate flags** — boolean, string, integer, and JSON flags, with per-rule return values and weighted variant distributions (60/30/10 splits) for A/B/n tests.
- **Targeting & rollouts** — attribute-based [targeting rules](/guide/concepts#targeting-rules) (string and numeric operators), reusable [segments](/guide/segments), sticky percentage rollouts, and [prerequisite flags](/guide/concepts#prerequisite-flags).
- **Analytics & experimentation** — impression tracking, [exposure dashboards](/guide/experimentation#exposure-dashboards), and an [A/B testing beta](/guide/experimentation#a-b-testing) that measures conversion goals and reports statistical significance.
- **Governance** — [RBAC](/guide/concepts#users-and-roles), an [audit log](/guide/governance#audit-log), [change-request approvals](/guide/governance#change-requests-approvals), [scheduled changes](/guide/governance#scheduled-changes), and personal access tokens.
- **A full ecosystem** — native SDKs (Node.js, Web/WASM, React Native, Flutter; Python, OpenFeature and Express/Fastify/NestJS integrations <Since v="0.2.0" />), a [type-safe codegen CLI](/ecosystem/cli), [edge evaluation](/ecosystem/edge), [SSR/bootstrap helpers](/ecosystem/ssr), and infrastructure-as-code via a [Terraform provider](/ecosystem/terraform) and [Kubernetes operator](/ecosystem/kubernetes).

## Local versus remote evaluation

This compares two architectures, not products: many hosted services also offer local-evaluation
SDKs. Check any vendor's current documentation before comparing it with Checkgate.

| | Checkgate (local evaluation) | Remote evaluation |
|---|---|---|
| Where a flag is decided | In your process, from an in-memory copy | On the flag service, per request |
| Cost of one check | 84–456 ns median in the Rust core, 0.8–4 µs through the Node.js SDK ([v0.1.31 benchmarks](/performance/benchmarks)) | A network round trip |
| When the flag server is unreachable | Last known flags keep answering | Depends on the client's fallback |
| How changes arrive | SSE push: p50 12–39 ms for 1–1,000 clients; p99 55–198 ms pooled, 84 ms median run at 1,000 ([benchmarks](/performance/benchmarks)) | Next request sees the change |
| What clients receive | The environment's rules (browser and mobile SDKs too) | Only the answers |
| Hosting | Self-hosted, Apache 2.0 | Varies |

Checkgate's own limits: experiments are a beta (goal events and a two-proportion significance
test), React Native's iOS build is not certified, the Flutter SDK ships no compiled engine, and
several 0.2.0 packages are not published yet — see [Which SDK do I need?](/sdks/#status-by-package).

## What Checkgate Is Not

- **Not a managed service** — you operate the server yourself (a single Docker image covers it).
- **Not a full product-analytics warehouse** — Checkgate records evaluation impressions and conversion events for exposure and A/B analysis, but it is not a replacement for a dedicated BI/analytics stack. (Warehouse export is [on the roadmap](/roadmap).)

## Who Is It For?

- Teams that want **full control** over their feature-flag infrastructure.
- Applications where **evaluation latency matters** — hot paths, mobile apps, edge/SSR, real-time systems.
- Organizations with **data-residency requirements** that prohibit sending user data to third parties.
- Companies replacing a **per-seat / per-MAU SaaS** flag vendor with a flat-cost, self-hosted alternative — see the [migration guide](/enterprise-setup).
- Developers who prefer **open source** and want to read, extend, and own the system.
