import { defineConfig, type DefaultTheme, type HeadConfig } from "vitepress";
import { release } from "./data/release.ts";

const SITE = "https://checkgate-sh.github.io/checkgate/";
const TITLE = "Checkgate";
const DESCRIPTION =
  "Open-source, self-hosted feature management with local evaluation and real-time synchronization. Feature flags evaluated in-process by Node.js, Python, browser, React Native and Flutter SDKs.";
const OG_IMAGE = `${SITE}og-image.png`;

// ── Sidebars: one per developer journey, chosen by path ──────────────────────
const guideSidebar: DefaultTheme.SidebarItem[] = [
  {
    text: "Getting started",
    items: [
      { text: "What is Checkgate?", link: "/guide/what-is-checkgate" },
      { text: "Five-minute quickstart", link: "/guide/quickstart" },
      { text: "Getting started", link: "/guide/getting-started" },
      { text: "Architecture", link: "/guide/architecture" },
    ],
  },
  {
    text: "Flags and targeting",
    items: [
      { text: "Core concepts", link: "/guide/concepts" },
      { text: "Segments", link: "/guide/segments" },
      { text: "Evaluation playground", link: "/guide/playground" },
    ],
  },
  {
    text: "Release control",
    items: [
      { text: "Governance and approvals", link: "/guide/governance" },
      { text: "Experimentation and analytics", link: "/guide/experimentation" },
      { text: "Dashboard tour", link: "/guide/dashboard" },
    ],
  },
  {
    text: "Project",
    items: [{ text: "Roadmap", link: "/roadmap" }],
  },
];

const sdkSidebar: DefaultTheme.SidebarItem[] = [
  {
    text: "Choose an SDK",
    items: [{ text: "Which SDK do I need?", link: "/sdks/" }],
  },
  {
    text: "Server SDKs",
    items: [
      { text: "Node.js", link: "/sdks/nodejs" },
      { text: "Python", link: "/sdks/python" },
    ],
  },
  {
    text: "Client SDKs",
    items: [
      { text: "Browser (WebAssembly)", link: "/sdks/web" },
      { text: "React Native", link: "/sdks/react-native" },
      { text: "Flutter", link: "/sdks/flutter" },
    ],
  },
  {
    text: "Standards and frameworks",
    items: [
      { text: "OpenFeature provider", link: "/sdks/openfeature" },
      { text: "Express, Fastify, NestJS", link: "/sdks/node-frameworks" },
    ],
  },
  {
    text: "Troubleshooting",
    items: [{ text: "SDK diagnostics", link: "/operations/sdk-diagnostics" }],
  },
];

const integrationsSidebar: DefaultTheme.SidebarItem[] = [
  {
    text: "Rendering and edge",
    items: [
      { text: "SSR and bootstrap", link: "/ecosystem/ssr" },
      { text: "Edge evaluation", link: "/ecosystem/edge" },
    ],
  },
  {
    text: "Automation",
    items: [
      { text: "AI agents (MCP)", link: "/ecosystem/mcp" },
      { text: "Terraform / OpenTofu", link: "/ecosystem/terraform" },
      { text: "Kubernetes operator", link: "/ecosystem/kubernetes" },
      { text: "Typed flags (CLI)", link: "/ecosystem/cli" },
    ],
  },
  {
    text: "Notifications",
    items: [{ text: "Slack and Teams alerts", link: "/ecosystem/chat-alerts" }],
  },
];

const operationsSidebar: DefaultTheme.SidebarItem[] = [
  {
    text: "Self-hosting",
    items: [
      { text: "Deploy with Docker", link: "/self-hosting" },
      { text: "Environment variables", link: "/self-hosting#environment-variables" },
      { text: "AWS and cloud", link: "/self-hosting#aws" },
      { text: "Enterprise setup and migration", link: "/enterprise-setup" },
    ],
  },
  {
    text: "Upgrading",
    items: [
      { text: "Upgrading to 0.2.0", link: "/operations/upgrading-to-0-2-0" },
      { text: "Upgrading to 0.1.31", link: "/operations/upgrading" },
    ],
  },
  {
    text: "Running Checkgate",
    items: [
      { text: "Reliability and sync", link: "/operations/reliability" },
      { text: "Configuration consistency", link: "/operations/configuration-consistency" },
      { text: "Deployment and recovery", link: "/operations/failure-recovery" },
      { text: "Observability", link: "/operations/observability" },
      { text: "SDK diagnostics", link: "/operations/sdk-diagnostics" },
    ],
  },
];

const referenceSidebar: DefaultTheme.SidebarItem[] = [
  {
    text: "API reference",
    items: [
      { text: "REST API", link: "/api-reference" },
      { text: "Authentication", link: "/api-reference#authentication" },
      { text: "Flag schema", link: "/api-reference#flag-schema" },
      { text: "SSE stream", link: "/api-reference#sse-stream" },
      { text: "Flags snapshot (poll fallback)", link: "/api-reference#flags-snapshot-poll-fallback" },
      { text: "Personal access tokens", link: "/api-reference#personal-access-tokens" },
    ],
  },
];

const performanceSidebar: DefaultTheme.SidebarItem[] = [
  {
    text: "Performance",
    items: [
      { text: "Public benchmarks", link: "/performance/benchmarks" },
      { text: "Infrastructure cost", link: "/performance/cost-efficiency" },
      { text: "Benchmark methodology", link: "/performance/benchmark-methodology" },
      { text: "October 2026 baseline", link: "/performance/baseline" },
    ],
  },
];

// ── Per-page SEO: canonical URL and Open Graph / Twitter tags ────────────────
function pageUrl(relativePath: string): string {
  const path = relativePath.replace(/(^|\/)index\.md$/, "$1").replace(/\.md$/, ".html");
  return SITE + path;
}

export default defineConfig({
  base: "/checkgate/",
  lang: "en-US",
  ignoreDeadLinks: [/^http:\/\/localhost/],
  // Only the public site is built from docs/. Raw benchmark results are data
  // for reproduction (linked on GitHub), not pages; release-engineering records
  // live outside the repository (internal_docs/, gitignored) and are excluded
  // here too in case a local copy lands under docs/.
  // scripts/check-docs-site.py verifies the built output against this policy.
  srcExclude: ["releases/**", "performance/results/**", "**/README.md"],
  title: TITLE,
  description: DESCRIPTION,
  sitemap: { hostname: SITE },

  head: [
    // `head` entries are not prefixed with `base`, so these carry it.
    ["link", { rel: "icon", type: "image/svg+xml", href: "/checkgate/favicon.svg" }],
    ["link", { rel: "icon", type: "image/png", href: "/checkgate/favicon.png" }],
    ["link", { rel: "apple-touch-icon", href: "/checkgate/apple-touch-icon.png" }],
    ["meta", { name: "theme-color", content: "#4f46e5", media: "(prefers-color-scheme: light)" }],
    ["meta", { name: "theme-color", content: "#0b1020", media: "(prefers-color-scheme: dark)" }],
    ["meta", { property: "og:site_name", content: TITLE }],
    ["meta", { property: "og:type", content: "website" }],
    ["meta", { property: "og:image", content: OG_IMAGE }],
    ["meta", { property: "og:image:width", content: "1200" }],
    ["meta", { property: "og:image:height", content: "630" }],
    [
      "meta",
      {
        property: "og:image:alt",
        content: "Checkgate — Ship with confidence. Roll out with control. Open-source, self-hosted feature management.",
      },
    ],
    ["meta", { name: "twitter:card", content: "summary_large_image" }],
    ["meta", { name: "twitter:image", content: OG_IMAGE }],
  ],

  transformHead({ pageData, title, description }) {
    const url = pageUrl(pageData.relativePath);
    const head: HeadConfig[] = [
      ["link", { rel: "canonical", href: url }],
      ["meta", { property: "og:url", content: url }],
      ["meta", { property: "og:title", content: title }],
      ["meta", { property: "og:description", content: description }],
      ["meta", { name: "twitter:title", content: title }],
      ["meta", { name: "twitter:description", content: description }],
    ];
    if (pageData.relativePath === "index.md") {
      // Truthful structured data only: no ratings, offers or prices.
      const ld = {
        "@context": "https://schema.org",
        "@type": "SoftwareSourceCode",
        name: TITLE,
        description: DESCRIPTION,
        url: SITE,
        codeRepository: release.repo,
        license: "https://www.apache.org/licenses/LICENSE-2.0",
        programmingLanguage: ["Rust", "TypeScript", "Python", "Dart"],
        runtimePlatform: ["Node.js", "Python", "Web browsers", "React Native", "Flutter", "Docker"],
        version: release.latest,
      };
      head.push(["script", { type: "application/ld+json" }, JSON.stringify(ld)]);
    }
    return head;
  },

  themeConfig: {
    logo: { src: "/logo.svg", alt: "" },
    nav: [
      {
        text: "Get started",
        link: "/guide/quickstart",
        activeMatch: "^/guide/(quickstart|getting-started|what-is-checkgate)",
      },
      {
        text: "Guides",
        link: "/guide/concepts",
        activeMatch: "^/(guide/(?!quickstart|getting-started|what-is-checkgate)|roadmap)",
      },
      { text: "SDKs", link: "/sdks/", activeMatch: "^/sdks/" },
      { text: "Integrations", link: "/ecosystem/ssr", activeMatch: "^/ecosystem/" },
      {
        text: "Operations",
        link: "/self-hosting",
        activeMatch: "^/(self-hosting|enterprise-setup|operations/)",
      },
      { text: "API", link: "/api-reference", activeMatch: "^/api-reference" },
      { text: "Performance", link: "/performance/benchmarks", activeMatch: "^/performance/" },
      {
        text: `v${release.latest}`,
        items: [
          { text: `Release notes (v${release.latest})`, link: `${release.repo}/releases/latest` },
          { text: "Changelog", link: `${release.repo}/blob/main/CHANGELOG.md` },
          ...(release.next ? [{ text: `Upgrading to ${release.next}`, link: "/operations/upgrading-to-0-2-0" }] : []),
          { text: "Roadmap", link: "/roadmap" },
        ],
      },
    ],

    sidebar: {
      "/guide/": guideSidebar,
      "/roadmap": guideSidebar,
      "/sdks/": sdkSidebar,
      "/ecosystem/": integrationsSidebar,
      "/self-hosting": operationsSidebar,
      "/enterprise-setup": operationsSidebar,
      "/operations/": operationsSidebar,
      "/api-reference": referenceSidebar,
      "/performance/": performanceSidebar,
    },

    socialLinks: [{ icon: "github", link: release.repo, ariaLabel: "Checkgate on GitHub" }],

    editLink: {
      pattern: `${release.repo}/edit/dev/docs/:path`,
      text: "Edit this page on GitHub",
    },

    outline: { level: [2, 3] },

    footer: {
      message: "Released under the Apache License 2.0.",
      copyright: "Copyright © 2025 ThinkGrid Labs",
    },

    search: {
      provider: "local",
    },
  },
});
