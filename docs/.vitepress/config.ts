import { defineConfig } from "vitepress";

export default defineConfig({
  base: "/checkgate/",
  ignoreDeadLinks: [/^http:\/\/localhost/],
  // Only the public site is built from docs/. Raw benchmark results are data
  // for reproduction (linked on GitHub), not pages; release-engineering records
  // live outside the repository (internal_docs/, gitignored) and are excluded
  // here too in case a local copy lands under docs/.
  // scripts/check-docs-site.py verifies the built output against this policy.
  srcExclude: ["releases/**", "performance/results/**", "**/README.md"],
  title: "Checkgate",
  description:
    "Self-hosted feature flags with local SDK evaluation, real-time SSE updates, and a lightweight Rust-powered server.",

  head: [
    // `head` entries are not prefixed with `base`, so these carry it.
    ["link", { rel: "icon", type: "image/svg+xml", href: "/checkgate/favicon.svg" }],
    ["link", { rel: "icon", type: "image/png", href: "/checkgate/favicon.png" }],
    ["link", { rel: "apple-touch-icon", href: "/checkgate/apple-touch-icon.png" }],
    ["meta", { name: "og:type", content: "website" }],
    [
      "meta",
      {
        name: "og:title",
        content: "Checkgate — Self-Hosted Feature Flag Engine",
      },
    ],
    [
      "meta",
      {
        name: "og:description",
        content:
          "In-process feature flag evaluation for Node.js, React Native, Flutter, and browsers. Self-hosted, open-source, no vendor lock-in.",
      },
    ],
    [
      "meta",
      {
        name: "keywords",
        content:
          "feature flags, open source feature flags, self-hosted feature toggles, LaunchDarkly alternative, in-process feature flags, Rust feature flag",
      },
    ],
    ["meta", { name: "twitter:card", content: "summary_large_image" }],
  ],

  themeConfig: {
    logo: "/logo.svg",
    nav: [
      { text: "Guide", link: "/guide/getting-started" },
      { text: "SDKs", link: "/sdks/" },
      { text: "Ecosystem", link: "/ecosystem/cli" },
      { text: "Self-Hosting", link: "/self-hosting" },
      { text: "API", link: "/api-reference" },
      { text: "Roadmap", link: "/roadmap" },
      {
        text: "GitHub",
        link: "https://github.com/checkgate-sh/checkgate",
      },
    ],

    sidebar: [
      {
        text: "Introduction",
        items: [
          { text: "What is Checkgate?", link: "/guide/what-is-checkgate" },
          { text: "Five-minute quickstart", link: "/guide/quickstart" },
          { text: "Getting Started", link: "/guide/getting-started" },
          { text: "Dashboard", link: "/guide/dashboard" },
          { text: "Evaluation playground", link: "/guide/playground" },
          { text: "Architecture", link: "/guide/architecture" },
          { text: "Roadmap", link: "/roadmap" },
        ],
      },
      {
        text: "Concepts & Features",
        items: [
          { text: "Core Concepts", link: "/guide/concepts" },
          { text: "Segments", link: "/guide/segments" },
          { text: "Experimentation & Analytics", link: "/guide/experimentation" },
          { text: "Governance", link: "/guide/governance" },
        ],
      },
      {
        text: "SDKs",
        items: [
          { text: "Which SDK do I need?", link: "/sdks/" },
          { text: "Node.js (NAPI)", link: "/sdks/nodejs" },
          { text: "Express, Fastify, NestJS", link: "/sdks/node-frameworks" },
          { text: "Web (WebAssembly)", link: "/sdks/web" },
          { text: "React Native (JSI)", link: "/sdks/react-native" },
          { text: "Flutter (FFI)", link: "/sdks/flutter" },
          { text: "Python (PyO3)", link: "/sdks/python" },
          { text: "OpenFeature (Node.js)", link: "/sdks/openfeature" },
        ],
      },
      {
        text: "Ecosystem & Integrations",
        items: [
          { text: "Slack & Teams Alerts", link: "/ecosystem/chat-alerts" },
          { text: "Type-Safe Schema CLI", link: "/ecosystem/cli" },
          { text: "Edge Evaluation", link: "/ecosystem/edge" },
          { text: "SSR / Bootstrap", link: "/ecosystem/ssr" },
          { text: "Terraform / OpenTofu", link: "/ecosystem/terraform" },
          { text: "Kubernetes Operator", link: "/ecosystem/kubernetes" },
          { text: "AI Agents (MCP)", link: "/ecosystem/mcp" },
        ],
      },
      {
        text: "Self-Hosting",
        items: [
          { text: "Docker", link: "/self-hosting" },
          { text: "Enterprise Setup", link: "/enterprise-setup" },
          { text: "Deployment & Recovery", link: "/operations/failure-recovery" },
          { text: "Reliability & Sync", link: "/operations/reliability" },
          { text: "Observability", link: "/operations/observability" },
          { text: "SDK diagnostics", link: "/operations/sdk-diagnostics" },
          {
            text: "Configuration Consistency",
            link: "/operations/configuration-consistency",
          },
          { text: "Upgrading to 0.1.31", link: "/operations/upgrading" },
          { text: "AWS & Cloud", link: "/self-hosting#aws" },
          {
            text: "Environment Variables",
            link: "/self-hosting#environment-variables",
          },
        ],
      },
      {
        text: "Performance",
        items: [
          { text: "Public Benchmarks", link: "/performance/benchmarks" },
          { text: "Infrastructure Cost", link: "/performance/cost-efficiency" },
          {
            text: "Benchmark Methodology",
            link: "/performance/benchmark-methodology",
          },
        ],
      },
      {
        text: "Reference",
        items: [
          { text: "REST API", link: "/api-reference" },
          { text: "Flag Schema", link: "/api-reference#flag-schema" },
          { text: "SSE Stream", link: "/api-reference#sse-stream" },
        ],
      },
    ],

    socialLinks: [
      { icon: "github", link: "https://github.com/checkgate-sh/checkgate" },
    ],

    footer: {
      message: "Released under the Apache License 2.0.",
      copyright: "Copyright © 2025 ThinkGrid Labs",
    },

    search: {
      provider: "local",
    },
  },
});
