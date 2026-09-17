---
layout: home
title: "Checkgate — Feature Flags Without the Round-Trip."
description: "Open-source, self-hosted feature flag engine that evaluates flags locally in sub-microseconds — no network call per evaluation. Native SDKs for Node.js, Web, React Native, and Flutter."
hero:
  name: "Checkgate"
  text: "Feature Flags Without the Round-Trip."
  tagline: "Flags evaluated in-process. Updates pushed in under 50 ms. No vendor, no round-trips, no latency tax."
  image:
    src: /logo.svg
    alt: Checkgate Logo
  actions:
    - theme: brand
      text: Get Started
      link: /guide/getting-started
    - theme: alt
      text: View on GitHub
      link: https://github.com/checkgate-sh/checkgate

features:
  - icon: ⚡
    title: Sub-Microsecond Evaluation
    details: Flags are evaluated entirely in-process from an in-memory store. Zero network latency on every isEnabled() call — no HTTP round-trips, no polling.

  - icon: 🏠
    title: Fully Self-Hosted
    details: Deploy on your own infrastructure with Docker. Your flag data never leaves your servers. Single binary, PostgreSQL, and Redis are all you need.

  - icon: 🔄
    title: Real-Time Updates via SSE
    details: Flag changes propagate instantly to all connected SDK clients through Server-Sent Events. No polling interval — changes land in milliseconds.

  - icon: 🎯
    title: Advanced Targeting Rules
    details: Target users by any attribute — email domain, plan, region, custom properties. Reusable segments, string and numeric operators, prerequisite flags, and sticky percentage rollouts.

  - icon: 🧪
    title: A/B Testing & Analytics
    details: Multivariate flags with weighted splits, impression tracking, exposure dashboards, and an experiments beta that measures conversion goals with statistical significance.

  - icon: 🦀
    title: Rust-Powered Core
    details: One evaluation engine written in Rust, compiled to native code (NAPI for Node.js, WASM for browsers, FFI for Flutter/React Native) — identical flag decisions everywhere.

  - icon: 🧰
    title: A Full Ecosystem
    details: Native SDKs, a type-safe codegen CLI, edge evaluation (Cloudflare Workers, Fly.io), SSR/bootstrap helpers, and infrastructure-as-code via a Terraform provider and Kubernetes operator.

  - icon: 🔓
    title: Open Source & Apache 2.0 Licensed
    details: No black boxes, no usage limits, no surprise pricing. Fork it, extend it, and own your feature flag infrastructure completely.
---
