---
title: "What is Checkgate? — Feature Flags Without the Round-Trip"
description: "Learn how Checkgate eliminates per-evaluation network latency by running the flag decision in-process, and how it compares to LaunchDarkly and Statsig."
---

# What is Checkgate?

Checkgate is a **self-hosted, open-source feature flag engine** built for teams that need fast, reliable flag evaluation without sending data to a third-party SaaS vendor.

## The Problem

Most feature flag services evaluate flags on their servers — meaning every `isEnabled()` call is a network request. This introduces:

- **Latency** — even a fast remote call adds 5–50ms per flag check
- **Reliability risk** — if the flag service is down, your flag evaluation breaks
- **Vendor lock-in** — migrating away is painful and expensive
- **Data privacy concerns** — user attributes are sent to an external service

## The Checkgate Approach

Checkgate takes a different approach: **local evaluation**.

1. Your server runs the Checkgate control plane (a single Rust binary)
2. Each SDK connects once via SSE and receives the full flag set
3. All evaluation happens **in-process, in memory** — no network calls
4. When flags change, the server pushes deltas via SSE instantly

The result: **in-process flag evaluation** (well under a microsecond in the Rust core) with real-time updates and zero external dependencies at evaluation time.

## What You Get

Checkgate is a complete feature-flag platform, not just a toggle store:

- **Multivariate flags** — boolean, string, integer, and JSON flags, with per-rule return values and weighted variant distributions (60/30/10 splits) for A/B/n tests.
- **Targeting & rollouts** — attribute-based [targeting rules](/guide/concepts#targeting-rules) (string and numeric operators), reusable [segments](/guide/segments), sticky percentage rollouts, and [prerequisite flags](/guide/concepts#prerequisite-flags).
- **Analytics & experimentation** — impression tracking, [exposure dashboards](/guide/experimentation#exposure-dashboards), and an [A/B testing beta](/guide/experimentation#a-b-testing) that measures conversion goals and reports statistical significance.
- **Governance** — [RBAC](/guide/concepts#users-and-roles), an [audit log](/guide/governance#audit-log), [change-request approvals](/guide/governance#change-requests), [scheduled changes](/guide/governance#scheduled-changes), and personal access tokens.
- **A full ecosystem** — native SDKs (Node.js, Web/WASM, React Native, Flutter), a [type-safe codegen CLI](/ecosystem/cli), [edge evaluation](/ecosystem/edge), [SSR/bootstrap helpers](/ecosystem/ssr), and infrastructure-as-code via a [Terraform provider](/ecosystem/terraform) and [Kubernetes operator](/ecosystem/kubernetes).

## How It Compares

| | Checkgate | LaunchDarkly / Statsig |
|---|---|---|
| Evaluation location | In-process (local) | Remote HTTP / local with proprietary SDK |
| Evaluation latency | 0.1–0.5 µs in the Rust core, a few µs through the Node.js SDK ([benchmarks](/performance/benchmarks)) | A network round trip per check when evaluated remotely |
| Update propagation | SSE push: p50 13–38 ms, p99 21–98 ms for 1–1,000 clients ([benchmarks](/performance/benchmarks)) | Polling or streaming, depending on the SDK |
| Self-hosted | Yes | No (or enterprise-only) |
| Open source | Apache 2.0 | Closed source |
| Vendor lock-in | None | High |
| A/B testing | Beta (goal events + significance) | Yes |
| Infrastructure as code | Terraform + Kubernetes operator | Terraform (managed) |
| Pricing | Free — your infra cost only | Per-seat / per-MAU |

## What Checkgate Is Not

- **Not a managed service** — you operate the server yourself (a single Docker image covers it).
- **Not a full product-analytics warehouse** — Checkgate records evaluation impressions and conversion events for exposure and A/B analysis, but it is not a replacement for a dedicated BI/analytics stack. (Warehouse export is [on the roadmap](/roadmap).)

## Who Is It For?

- Teams that want **full control** over their feature-flag infrastructure.
- Applications where **evaluation latency matters** — hot paths, mobile apps, edge/SSR, real-time systems.
- Organizations with **data-residency requirements** that prohibit sending user data to third parties.
- Companies replacing a **per-seat / per-MAU SaaS** flag vendor with a flat-cost, self-hosted alternative — see the [migration guide](/enterprise-setup).
- Developers who prefer **open source** and want to read, extend, and own the system.
