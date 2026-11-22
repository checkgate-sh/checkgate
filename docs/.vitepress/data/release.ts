// Release status for the docs site, in one place.
//
// Pages never hard-code "unreleased" or "not on npm": they use <Since>,
// <ReleaseNotice> and <SdkMatrix>, which read this file. When 0.2.0 is
// published, set `latest` to '0.2.0', `next` to null, and update each
// package's `registry` to what the registries actually serve (check with
// `npm view`, PyPI and pub.dev; never assume a publish succeeded).

export const release = {
  /** Newest version published to npm, pub.dev, ghcr.io and GitHub Releases. */
  latest: '0.1.31',
  /** The version in development on `dev`, or null when nothing is pending. */
  next: '0.2.0' as string | null,
  repo: 'https://github.com/checkgate-sh/checkgate',
}

/** Compare dotted versions: -1, 0, 1. */
export function compareVersions(a: string, b: string): number {
  const pa = a.split('.').map(Number)
  const pb = b.split('.').map(Number)
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0)
    if (d) return d < 0 ? -1 : 1
  }
  return 0
}

/** True when `version` is newer than the latest published release. */
export const isUnreleased = (version: string) => compareVersions(version, release.latest) > 0

export type Registry =
  | { kind: 'npm' | 'pub' | 'pypi'; version: string }
  | { kind: 'unpublished'; note: string }
  | { kind: 'source'; note: string }

export interface Package {
  id: string
  name: string
  platform: string
  binding: string
  doc: string
  /** First version that ships it. */
  since: string
  registry: Registry
  /** Install command once published (or for a registry package). */
  install?: string
  limits?: string
  group: 'sdk' | 'framework' | 'tooling'
}

export const packages: Package[] = [
  { id: 'node', name: '@checkgate/node', platform: 'Node.js', binding: 'NAPI', doc: '/sdks/nodejs', since: '0.1.0',
    registry: { kind: 'npm', version: '0.1.31' }, install: 'npm install @checkgate/node', group: 'sdk',
    limits: 'Node.js runtime only (not edge runtimes). getDetails(), getHealth() and onStatusChange() arrive in 0.2.0.' },
  { id: 'python', name: 'checkgate', platform: 'Python', binding: 'PyO3', doc: '/sdks/python', since: '0.2.0',
    registry: { kind: 'source', note: 'Not on PyPI yet: build from the repository (needs Rust). Do not install a package named checkgate from PyPI until the project announces it.' },
    group: 'sdk', limits: 'CPython 3.11+. No impression reporting or track() yet.' },
  { id: 'web', name: '@checkgate/web', platform: 'Browser', binding: 'WebAssembly', doc: '/sdks/web', since: '0.1.0',
    registry: { kind: 'npm', version: '0.1.31' }, install: 'npm install @checkgate/web', group: 'sdk' },
  { id: 'react-native', name: '@checkgate/react-native', platform: 'React Native', binding: 'JSI', doc: '/sdks/react-native', since: '0.1.0',
    registry: { kind: 'npm', version: '0.1.31' }, install: 'npm install @checkgate/react-native', group: 'sdk',
    limits: 'Android native build verified in CI; iOS not certified. Builds the Rust engine in your app build.' },
  { id: 'flutter', name: 'checkgate_flutter', platform: 'Flutter', binding: 'Dart FFI', doc: '/sdks/flutter', since: '0.1.0',
    registry: { kind: 'pub', version: '0.1.31' }, install: 'flutter pub add checkgate_flutter', group: 'sdk',
    limits: 'Bundles no compiled engine: you build and link the Rust engine yourself.' },
  { id: 'openfeature', name: '@checkgate/openfeature-provider', platform: 'OpenFeature (Node.js server)', binding: 'over @checkgate/node', doc: '/sdks/openfeature', since: '0.2.0',
    registry: { kind: 'unpublished', note: 'Publishes to npm with 0.2.0; it needs @checkgate/node 0.2.' },
    install: 'npm install @checkgate/openfeature-provider @openfeature/server-sdk', group: 'framework' },
  { id: 'node-integrations', name: '@checkgate/node-integrations', platform: 'Express · Fastify · NestJS', binding: 'over @checkgate/node', doc: '/sdks/node-frameworks', since: '0.2.0',
    registry: { kind: 'unpublished', note: 'Publishes to npm with 0.2.0.' },
    install: 'npm install @checkgate/node @checkgate/node-integrations', group: 'framework',
    limits: 'Node.js 22.12+; Express 5, Fastify 5, NestJS 12.' },
  { id: 'ssr', name: '@checkgate/ssr', platform: 'SSR / bootstrap', binding: 'with @checkgate/web', doc: '/ecosystem/ssr', since: '0.1.0',
    registry: { kind: 'npm', version: '0.1.31' }, install: 'npm install @checkgate/web @checkgate/ssr', group: 'framework' },
  { id: 'edge', name: '@checkgate/edge', platform: 'Edge runtimes', binding: 'with @checkgate/web', doc: '/ecosystem/edge', since: '0.1.0',
    registry: { kind: 'npm', version: '0.1.31' }, install: 'npm install @checkgate/edge @checkgate/web', group: 'framework' },
  { id: 'cli', name: '@checkgate/cli', platform: 'Typed flag accessors', binding: 'dev dependency', doc: '/ecosystem/cli', since: '0.1.0',
    registry: { kind: 'npm', version: '0.1.31' }, install: 'npm install -D @checkgate/cli', group: 'tooling' },
  { id: 'mcp', name: '@checkgate/mcp-server', platform: 'AI agents (MCP)', binding: 'stdio', doc: '/ecosystem/mcp', since: '0.2.0',
    registry: { kind: 'unpublished', note: 'Publishes to npm with 0.2.0; needs a 0.2.0 server.' }, group: 'tooling',
    limits: 'Read-only by default; can propose changes, never create, apply or approve.' },
  { id: 'terraform', name: 'terraform-provider-checkgate', platform: 'Terraform / OpenTofu', binding: 'Go', doc: '/ecosystem/terraform', since: '0.1.0',
    registry: { kind: 'source', note: 'Not on the Terraform Registry: build from source.' }, group: 'tooling' },
  { id: 'kubernetes', name: 'checkgate-operator', platform: 'Kubernetes', binding: 'Go', doc: '/ecosystem/kubernetes', since: '0.1.0',
    registry: { kind: 'source', note: 'v1alpha1; no published image: build from source.' }, group: 'tooling' },
]

export const pkg = (id: string) => {
  const p = packages.find((x) => x.id === id)
  if (!p) throw new Error(`docs/.vitepress/data/release.ts: unknown package id "${id}"`)
  return p
}

/** Short status text for tables and badges. */
export function statusLabel(p: Package): string {
  const r = p.registry
  if (r.kind === 'npm') return `npm ${r.version}`
  if (r.kind === 'pub') return `pub.dev ${r.version}`
  if (r.kind === 'pypi') return `PyPI ${r.version}`
  if (r.kind === 'unpublished') return `${p.since} · not yet on npm`
  return isUnreleased(p.since) ? `${p.since} · build from source` : 'build from source'
}
