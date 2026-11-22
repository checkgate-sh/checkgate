---
layout: home
title: "Checkgate — Open-Source, Self-Hosted Feature Flags with Local Evaluation"
titleTemplate: false
description: "Open-source, self-hosted feature management with local evaluation and real-time synchronization. Feature flags evaluated in-process for Node.js, Python, browsers, React Native and Flutter, kept current over SSE."
markdownStyles: false
---

<HomeHero />

<HomePreview />

<HomeSection id="how-it-works" eyebrow="How it works" title="Synchronization in the background. Evaluation in your process." lead="The server stores and distributes configuration. Your application's flag checks never wait on it: every SDK answers from a local copy that the server keeps current over Server-Sent Events.">

<HowItWorks />

</HomeSection>

<HomeSection id="features" eyebrow="Why Checkgate" title="Control every release, from first percent to full rollout." tint>

<div class="cg-grid-3">
<FeatureCard icon="Gauge" title="Local evaluation" link="/guide/architecture">
SDKs evaluate flags from memory with the same Rust engine, so a flag check makes no network request and keeps answering from the last known flags when the server is unreachable.
</FeatureCard>
<FeatureCard icon="RadioTower" title="Real-time updates" link="/operations/reliability">
Changes are pushed over one SSE connection per SDK, with heartbeats, reconnects with backoff, a polling fallback and per-flag revision ordering.
</FeatureCard>
<FeatureCard icon="Target" title="Advanced targeting" link="/guide/segments">
Rules on any user attribute, reusable segments, prerequisite flags and multivariate flags — the same answer on a server, in a browser and on a phone.
</FeatureCard>
<FeatureCard icon="SlidersHorizontal" title="Controlled rollouts" link="/guide/concepts">
Deterministic, sticky percentage rollouts per flag, scheduled changes, and per-environment configuration you can compare and promote.
</FeatureCard>
<FeatureCard icon="Server" title="Self-hosted" link="/self-hosting">
One Rust server with PostgreSQL and Redis on your infrastructure, Apache 2.0 licensed. No third-party service sees your flags or users.
</FeatureCard>
<FeatureCard icon="ShieldCheck" title="Governance and observability" link="/guide/governance">
Change requests with self-approval blocked, an audit log, scoped tokens, webhooks, Prometheus metrics and per-environment SDK health.
</FeatureCard>
</div>

</HomeSection>

<HomeSection id="stack" eyebrow="Built for your stack" title="One engine. Every SDK gives the same answer." lead="Each SDK embeds the shared Rust evaluation core and checks the same test vectors. Pick your runtime; the status shows what you can install today.">

<StackSelector :tabs="[
  { id: 'node', label: 'Node.js', pkg: 'node' },
  { id: 'python', label: 'Python', pkg: 'python' },
  { id: 'web', label: 'Browser', pkg: 'web' },
  { id: 'rn', label: 'React Native', pkg: 'react-native' },
  { id: 'flutter', label: 'Flutter', pkg: 'flutter' },
  { id: 'openfeature', label: 'OpenFeature', pkg: 'openfeature' },
]">
<template #node>

```ts
// npm install @checkgate/node
import { CheckgateClient } from '@checkgate/node'

const client = new CheckgateClient({
  serverUrl: process.env.CHECKGATE_URL!,
  sdkKey: process.env.CHECKGATE_SDK_KEY!,
})
await client.connect()

// Synchronous, in-process: no network request per check
const enabled = client.isEnabled('new-checkout', 'user-123', { plan: 'pro' })
```

</template>
<template #python>

```python
# Not on PyPI yet: build from the repository (see the Python SDK docs)
import os
from checkgate import CheckgateClient

client = CheckgateClient(os.environ["CHECKGATE_URL"], os.environ["CHECKGATE_SDK_KEY"])
client.connect(timeout=10)

# Synchronous and thread-safe; syncing runs on a background thread
enabled = client.is_enabled("new-checkout", "user-123", {"plan": "pro"})
```

</template>
<template #web>

```ts
// npm install @checkgate/web
import { CheckgateWeb } from '@checkgate/web'

const client = new CheckgateWeb({
  serverUrl: import.meta.env.VITE_CHECKGATE_URL,
  sdkKey: import.meta.env.VITE_CHECKGATE_SDK_KEY,
})
await client.connect()

// Evaluated in the browser by WebAssembly
const enabled = client.isEnabled('new-homepage', 'user-123', { plan: 'pro' })
```

</template>
<template #rn>

```ts
// npm install @checkgate/react-native
import { CheckgateNativeClient } from '@checkgate/react-native'

const client = new CheckgateNativeClient({
  serverUrl: 'https://flags.yourcompany.com',
  sdkKey: 'your-sdk-key',
})

// Once, at app start-up; flags then update live over SSE
await client.connect()

// A direct, synchronous call into the native engine through JSI
const enabled = client.isEnabled('new-onboarding', 'user-123', { plan: 'pro' })
```

</template>
<template #flutter>

```dart
// flutter pub add checkgate_flutter
import 'package:checkgate_flutter/checkgate_flutter.dart';

final flags = CheckgateClient(
  serverUrl: 'https://flags.yourcompany.com',
  sdkKey: 'your-sdk-key',
);
await flags.connect();

// Evaluated through Dart FFI by the Rust engine you link into the app
final enabled = flags.isEnabled('new-onboarding', 'user-123', {'plan': 'pro'});
```

</template>
<template #openfeature>

```ts
// npm install @checkgate/openfeature-provider @openfeature/server-sdk
import { OpenFeature } from '@openfeature/server-sdk'
import { CheckgateProvider } from '@checkgate/openfeature-provider'

await OpenFeature.setProviderAndWait(new CheckgateProvider({
  serverUrl: process.env.CHECKGATE_URL!,
  sdkKey: process.env.CHECKGATE_SDK_KEY!,
}))
const client = OpenFeature.getClient()

const enabled = await client.getBooleanValue('new-checkout', false, {
  targetingKey: 'user-123',
  plan: 'pro',
})
```

</template>
</StackSelector>

<p class="cg-home-note">Also: <a href="./sdks/node-frameworks.html">Express, Fastify and NestJS</a>, <a href="./ecosystem/ssr.html">SSR bootstrap</a>, <a href="./ecosystem/edge.html">edge runtimes</a>, <a href="./ecosystem/terraform.html">Terraform</a>, <a href="./ecosystem/kubernetes.html">Kubernetes</a> and <a href="./ecosystem/mcp.html">AI agents over MCP</a>. <a href="./sdks/#status-by-package">Compare every SDK and its status</a>.</p>

</HomeSection>

<HomeSection id="performance" eyebrow="Performance" title="Measured, with the conditions attached." lead="Figures from the public benchmark suite, run on one laptop with the server held to a single vCPU. Each value names its statistic and links to every run." tint>

<BenchmarkStats />

</HomeSection>

<HomeSection id="self-hosting" eyebrow="Deployment and ownership" title="Run it where your data lives.">

<HomeDeploy />

</HomeSection>

<HomeCta />

<style>
.cg-grid-3 {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 16px;
}
@media (max-width: 960px) {
  .cg-grid-3 { grid-template-columns: repeat(2, minmax(0, 1fr)); }
}
@media (max-width: 640px) {
  .cg-grid-3 { grid-template-columns: 1fr; }
}
.cg-home-note {
  max-width: 760px;
  margin: 24px auto 0;
  text-align: center;
  font-size: 14.5px;
  line-height: 1.7;
  color: var(--cg-ink-2);
}
.cg-home-note a {
  font-weight: 600;
  color: var(--cg-accent);
}
</style>
