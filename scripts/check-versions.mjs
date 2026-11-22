#!/usr/bin/env node
// Release version consistency check.
//
//   node scripts/check-versions.mjs            check; exit 1 on any mismatch
//   node scripts/check-versions.mjs --json     the same, as a JSON report
//   node scripts/check-versions.mjs --list npm print the lockstep npm package
//                                              directories (used by bump-version.sh)
//
// The release version is Cargo.toml [workspace.package].version. Packages are
// DISCOVERED, not listed, so a package added later is checked (and bumped by
// scripts/bump-version.sh) without editing either script:
//
//   Rust    every workspace member inherits `version.workspace = true`; every
//           path dependency on a workspace crate requires exactly the release
//           version (a `^0.1.0` requirement stops resolving at 0.2.0); the
//           workspace crates in Cargo.lock carry the release version.
//   npm     every package.json outside node_modules/build output that is not
//           `"private": true` is publishable: its version, its package-lock.json
//           (top level and packages[""]) and every @checkgate/* dependency or
//           peer range must agree with the release version, and release.yml
//           must have a job that runs `npm publish` in its directory.
//           Private packages are internal and only listed, except LOCKSTEP
//           ones, which are versioned with the release.
//   Python  every pyproject.toml: version, the `__version__` constant, and a
//           PyPI publish step in release.yml.
//   Dart    every pubspec.yaml without `publish_to: none`: version, a CHANGELOG
//           heading for it, and a pub.dev publish step in release.yml.
//
// Packages that are deliberately versioned on their own are EXCEPTIONS below,
// with the reason; anything else that disagrees fails the check.

import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const rel = (p) => path.relative(root, p).split(path.sep).join('/')
const read = (p) => readFileSync(path.join(root, p), 'utf8')

const SKIP_DIRS = new Set([
  'node_modules', 'target', '.git', 'dist', 'build', '.dart_tool', 'internal_docs',
  '.venv', 'venv', '.vitepress', 'rust-core', 'pkg', '.next',
])

// Private packages that are still versioned with the release.
const LOCKSTEP_PRIVATE = new Set(['dashboard'])

// Deliberate exceptions: directory -> reason. They are reported, not checked.
const EXCEPTIONS = {
  'docs': 'documentation site (private, not released)',
  'quickstart/node': 'quickstart example (private); depends on the published @checkgate/node',
  'edge/examples/cloudflare-worker': 'example project (private)',
  'integrations/checkgate-go': 'Go module, source only: no registry, no version field (Go versions modules by tag)',
  'integrations/terraform-provider-checkgate': 'Go module, source only: not on the Terraform Registry',
  'integrations/kubernetes-operator': 'Go module, source only: no published image or chart',
}

function walk(dir, names, out = []) {
  for (const entry of readdirSync(dir)) {
    if (SKIP_DIRS.has(entry)) continue
    const full = path.join(dir, entry)
    let st
    try { st = statSync(full) } catch { continue }
    if (st.isDirectory()) walk(full, names, out)
    else if (names.includes(entry)) out.push(full)
  }
  return out
}

const errors = []
const packages = []
const err = (where, msg) => errors.push(`${where}: ${msg}`)

// --- Release version -------------------------------------------------------
const cargoToml = read('Cargo.toml')
const wsPkg = cargoToml.split(/^\[workspace\.package\]$/m)[1]?.split(/^\[/m)[0] ?? ''
const VERSION = wsPkg.match(/^version\s*=\s*"([^"]+)"/m)?.[1]
if (!VERSION) {
  console.error('Cargo.toml: no [workspace.package] version')
  process.exit(2)
}

// --- Minimal semver range check (the forms this repository uses) ------------
const parse = (v) => v.replace(/^[v=]/, '').split('-')[0].split('.').map(Number)
const cmp = (a, b) => {
  const x = parse(a); const y = parse(b)
  for (let i = 0; i < 3; i++) if ((x[i] ?? 0) !== (y[i] ?? 0)) return (x[i] ?? 0) < (y[i] ?? 0) ? -1 : 1
  return 0
}
function satisfies(version, range) {
  return range.split('||').some((alt) => alt.trim().split(/\s+/).every((c) => {
    if (c === '*' || c === '') return true
    let m
    if ((m = c.match(/^\^(.+)$/))) {
      const [maj, min] = parse(m[1])
      const upper = maj > 0 ? `${maj + 1}.0.0` : `0.${min + 1}.0`
      return cmp(version, m[1]) >= 0 && cmp(version, upper) < 0
    }
    if ((m = c.match(/^~(.+)$/))) {
      const [maj, min] = parse(m[1])
      return cmp(version, m[1]) >= 0 && cmp(version, `${maj}.${min + 1}.0`) < 0
    }
    if ((m = c.match(/^(>=|<=|>|<)(.+)$/))) {
      const r = cmp(version, m[2])
      return { '>=': r >= 0, '<=': r <= 0, '>': r > 0, '<': r < 0 }[m[1]]
    }
    return cmp(version, c) === 0
  }))
}

const releaseYml = existsSync(path.join(root, '.github/workflows/release.yml'))
  ? read('.github/workflows/release.yml') : ''
// Jobs of release.yml, split on two-space-indented job keys.
const releaseJobs = releaseYml.split(/^ {2}(?=[a-z0-9-]+:\s*$)/m).slice(1)
const jobPublishes = (dir, cmd) => releaseJobs.some((j) =>
  new RegExp(`working-directory:\\s*${dir.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&')}\\s*$`, 'm').test(j) && j.includes(cmd))

// --- Rust --------------------------------------------------------------------
const members = [...(cargoToml.match(/members\s*=\s*\[([^\]]*)\]/)?.[1] ?? '').matchAll(/"([^"]+)"/g)].map((m) => m[1])
const crateNames = new Map()
for (const m of members) {
  const file = `${m}/Cargo.toml`
  const t = read(file)
  const name = t.match(/^\[package\][^[]*?^name\s*=\s*"([^"]+)"/ms)?.[1]
  crateNames.set(name, m)
  const publish = !/^publish\s*=\s*false/m.test(t)
  if (!/^version\.workspace\s*=\s*true/m.test(t)) err(file, `does not inherit version.workspace (release ${VERSION})`)
  packages.push({ ecosystem: 'cargo', name, dir: m, version: VERSION, registry: 'none (workspace crate, not published to crates.io)', publish: false, crateFlagPublish: publish })
}
for (const m of members) {
  const file = `${m}/Cargo.toml`
  for (const [, dep, ver] of read(file).matchAll(/^([A-Za-z0-9_-]+)\s*=\s*\{[^}]*?version\s*=\s*"([^"]+)"[^}]*?path\s*=/gm)) {
    if (crateNames.has(dep) && ver !== VERSION) err(file, `${dep} requires "${ver}", expected "${VERSION}"`)
  }
}
if (existsSync(path.join(root, 'Cargo.lock'))) {
  const lock = read('Cargo.lock')
  for (const name of crateNames.keys()) {
    const v = lock.match(new RegExp(`\\[\\[package\\]\\]\\nname = "${name}"\\nversion = "([^"]+)"\\n(?!source)`))?.[1]
    if (v !== VERSION) err('Cargo.lock', `${name} is ${v ?? 'missing'}, expected ${VERSION} (run \`cargo update --workspace\`)`)
  }
}

// --- npm ---------------------------------------------------------------------
const npmLockstep = []
for (const file of walk(root, ['package.json'])) {
  const dir = rel(path.dirname(file))
  const pkg = JSON.parse(readFileSync(file, 'utf8'))
  if (EXCEPTIONS[dir]) {
    packages.push({ ecosystem: 'npm', name: pkg.name, dir, version: pkg.version ?? null, registry: 'internal', publish: false, exception: EXCEPTIONS[dir] })
    continue
  }
  const isPrivate = pkg.private === true
  if (isPrivate && !LOCKSTEP_PRIVATE.has(dir)) {
    err(`${dir}/package.json`, `private package not classified: add it to LOCKSTEP_PRIVATE or EXCEPTIONS in scripts/check-versions.mjs`)
    continue
  }
  npmLockstep.push(dir)
  packages.push({ ecosystem: 'npm', name: pkg.name, dir, version: pkg.version, registry: isPrivate ? 'internal (lockstep)' : 'npm', publish: !isPrivate })
  if (pkg.version !== VERSION) err(`${dir}/package.json`, `version ${pkg.version}, expected ${VERSION}`)
  const lockFile = path.join(path.dirname(file), 'package-lock.json')
  if (existsSync(lockFile)) {
    const lock = JSON.parse(readFileSync(lockFile, 'utf8'))
    if (lock.version !== VERSION || (lock.packages?.[''] && lock.packages[''].version !== VERSION)) {
      err(`${dir}/package-lock.json`, `version ${lock.version}/${lock.packages?.['']?.version}, expected ${VERSION}`)
    }
  }
  for (const field of ['dependencies', 'peerDependencies', 'optionalDependencies']) {
    for (const [dep, range] of Object.entries(pkg[field] ?? {})) {
      if (!dep.startsWith('@checkgate/') || range.startsWith('file:') || range.startsWith('workspace:')) continue
      if (!satisfies(VERSION, range)) err(`${dir}/package.json`, `${field}.${dep} "${range}" does not accept ${VERSION}`)
    }
  }
  if (!isPrivate) {
    if (!pkg.name?.startsWith('@checkgate/')) err(`${dir}/package.json`, `publishable package outside the @checkgate scope: ${pkg.name}`)
    if (!jobPublishes(dir, 'npm publish')) err('.github/workflows/release.yml', `no job runs \`npm publish\` in ${dir} (${pkg.name})`)
    if (!existsSync(path.join(path.dirname(file), 'README.md'))) err(dir, 'publishable package has no README.md')
  }
}

// --- Python ------------------------------------------------------------------
for (const file of walk(root, ['pyproject.toml'])) {
  const dir = rel(path.dirname(file))
  const t = readFileSync(file, 'utf8')
  const name = t.match(/^\[project\][^[]*?^name\s*=\s*"([^"]+)"/ms)?.[1]
  const v = t.match(/^\[project\][^[]*?^version\s*=\s*"([^"]+)"/ms)?.[1]
  packages.push({ ecosystem: 'pypi', name, dir, version: v, registry: 'PyPI', publish: true })
  if (v !== VERSION) err(`${dir}/pyproject.toml`, `version ${v}, expected ${VERSION}`)
  for (const py of walk(path.dirname(file), ['__init__.py', '_client.py', '_version.py'])) {
    const m = readFileSync(py, 'utf8').match(/^__version__\s*=\s*"([^"]+)"/m)
    if (m && m[1] !== VERSION) err(rel(py), `__version__ ${m[1]}, expected ${VERSION}`)
  }
  const publishes = releaseJobs.some((j) => j.includes('pypa/gh-action-pypi-publish'))
  if (!publishes) err('.github/workflows/release.yml', `no PyPI publish step for ${name} (${dir})`)
}

// --- Dart --------------------------------------------------------------------
for (const file of walk(root, ['pubspec.yaml'])) {
  const dir = rel(path.dirname(file))
  const t = readFileSync(file, 'utf8')
  if (/^publish_to:\s*["']?none/m.test(t)) continue
  const name = t.match(/^name:\s*(\S+)/m)?.[1]
  const v = t.match(/^version:\s*(\S+)/m)?.[1]
  packages.push({ ecosystem: 'pub', name, dir, version: v, registry: 'pub.dev', publish: true })
  if (v !== VERSION) err(`${dir}/pubspec.yaml`, `version ${v}, expected ${VERSION}`)
  const changelog = path.join(path.dirname(file), 'CHANGELOG.md')
  if (!existsSync(changelog) || !new RegExp(`^## ${VERSION.replace(/\./g, '\\.')}\\b`, 'm').test(readFileSync(changelog, 'utf8'))) {
    err(`${dir}/CHANGELOG.md`, `no "## ${VERSION}" entry (pub.dev shows this file)`)
  }
  if (!jobPublishes(dir, 'dart pub publish --force')) err('.github/workflows/release.yml', `no pub.dev publish job for ${dir}`)
}

// --- Root changelog ----------------------------------------------------------
if (!new RegExp(`^## \\[${VERSION.replace(/\./g, '\\.')}\\]`, 'm').test(read('CHANGELOG.md'))) {
  err('CHANGELOG.md', `no "## [${VERSION}]" entry`)
}

// --- Go modules (exceptions, listed for the inventory) -----------------------
for (const file of walk(root, ['go.mod'])) {
  const dir = rel(path.dirname(file))
  const mod = readFileSync(file, 'utf8').match(/^module\s+(\S+)/m)?.[1]
  packages.push({ ecosystem: 'go', name: mod, dir, version: null, registry: 'source only', publish: false, exception: EXCEPTIONS[dir] ?? null })
  if (!EXCEPTIONS[dir]) err(dir, 'Go module not classified: add it to EXCEPTIONS (or give it a release job)')
}

// --- Output ------------------------------------------------------------------
const argv = process.argv.slice(2)
if (argv[0] === '--list') {
  if (argv[1] === 'npm') console.log(npmLockstep.sort().join('\n'))
  else { console.error('usage: --list npm'); process.exit(2) }
  process.exit(0)
}
if (argv.includes('--json')) {
  console.log(JSON.stringify({ version: VERSION, ok: errors.length === 0, errors, packages }, null, 2))
} else {
  console.log(`Release version (Cargo.toml [workspace.package]): ${VERSION}\n`)
  const rows = packages.map((p) => [p.ecosystem, p.name ?? '?', p.dir, p.version ?? '-', p.exception ? `exception: ${p.exception}` : p.registry])
  const w = [0, 1, 2, 3].map((i) => Math.max(...rows.map((r) => r[i].length)))
  for (const r of rows) console.log(`  ${r.slice(0, 4).map((c, i) => c.padEnd(w[i])).join('  ')}  ${r[4]}`)
  console.log('')
  if (errors.length) {
    console.log(`${errors.length} problem(s):`)
    for (const e of errors) console.log(`  - ${e}`)
  } else {
    console.log('All release versions are consistent.')
  }
}
process.exit(errors.length ? 1 : 0)
