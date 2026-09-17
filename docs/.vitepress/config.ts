import { defineConfig } from "vitepress";

export default defineConfig({
  base: "/checkgate/",
  ignoreDeadLinks: [/^http:\/\/localhost/],
  title: "Checkgate",
  description:
    "The ultimate open-source feature flag engine. Achieve sub-microsecond feature toggle evaluation down to the local level with native SDKs. Self-hosted and fully private.",

  head: [
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
          "Sub-microsecond feature flag evaluation for Node.js, React Native, Flutter, and browsers. Self-hosted, open-source, no vendor lock-in.",
      },
    ],
    [
      "meta",
      {
        name: "keywords",
        content:
          "feature flags, open source feature flags, self-hosted feature toggles, LaunchDarkly alternative, sub-microsecond feature flags, Rust feature flag",
      },
    ],
    ["meta", { name: "twitter:card", content: "summary_large_image" }],
  ],

  themeConfig: {
    logo: "/checkgate_logo.png",
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
          { text: "Getting Started", link: "/guide/getting-started" },
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
          { text: "Web (WebAssembly)", link: "/sdks/web" },
          { text: "React Native (JSI)", link: "/sdks/react-native" },
          { text: "Flutter (FFI)", link: "/sdks/flutter" },
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
        ],
      },
      {
        text: "Self-Hosting",
        items: [
          { text: "Docker", link: "/self-hosting" },
          { text: "Enterprise Setup", link: "/enterprise-setup" },
          { text: "AWS & Cloud", link: "/self-hosting#aws" },
          {
            text: "Environment Variables",
            link: "/self-hosting#environment-variables",
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
