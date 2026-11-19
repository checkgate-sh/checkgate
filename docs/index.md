---
layout: home
title: "Checkgate — Feature Flags Without the Round-Trip."
description: "Open-source, self-hosted feature flag engine that evaluates flags locally — no network call per evaluation. Native SDKs for Node.js, Web, React Native, and Flutter."
hero:
  name: "Checkgate"
  text: "Feature Flags Without the Round-Trip."
  tagline: "Flags evaluated in-process. Updates pushed in tens of milliseconds. No vendor, no round-trips, no latency tax."
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
    title: In-Process Evaluation
    details: Flags are evaluated in-process from an in-memory store — well under a microsecond in the Rust core, about 1–4 µs through the Node.js SDK. No HTTP request per isEnabled() call.

  - icon: 🏠
    title: Fully Self-Hosted
    details: Deploy on your own infrastructure with Docker — no third-party service sees your flags or users. Single binary, PostgreSQL, and Redis are all you need. (Client-side SDKs download their environment's flag rules, as with any local-evaluation SDK.)

  - icon: 🔄
    title: Real-Time Updates via SSE
    details: Flag changes are pushed to connected SDK clients over one persistent Server-Sent Events connection each, typically within tens of milliseconds. SDKs fall back to polling only when the stream cannot be established.

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
