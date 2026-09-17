# Contributing to Checkgate

Thanks for your interest in contributing. This document covers how to get set up, the project conventions, and the process for submitting changes.

## Prerequisites

- Rust 1.88+
- Node.js 18+ (SDKs, CLI, docs)
- Bun 1.4+ (dashboard)
- Docker and Docker Compose (for integration tests)
- `wasm-pack` (browser SDK)
- `@napi-rs/cli` (Node.js SDK)

## Getting started

```bash
git clone https://github.com/checkgate-sh/checkgate.git
cd checkgate
docker compose up -d       # start Postgres + Redis
cargo build --workspace    # verify everything compiles
cargo test --workspace     # run all tests
```

## Project layout

```
core/               Rust evaluation engine (shared across all SDKs)
server/             Axum HTTP server (REST API + SSE stream)
sdks/
  nodejs/           NAPI native addon
  browser/          wasm-bindgen / wasm-pack
  react-native/     JSI C++ bridge
  flutter/          dart:ffi binding
dashboard/          Vite + React control-plane UI (Bun)
docs/               VitePress documentation
```

### Dashboard

The dashboard uses Bun as both package manager and runtime:

```bash
cd dashboard
bun install
bun run dev                # dev server on :5173, proxies /api to :3000
bun run build              # type-check + build into ../server/public
bun run test --run         # vitest suite
```

Use `bun run test`, not `bun test` — the latter invokes Bun's own test runner,
which does not understand the vitest setup in `src/setupTests.ts`.

## Making changes

1. Fork the repo and create a branch from `main`.
2. Keep changes focused — one logical change per PR.
3. Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` before pushing.
4. Add or update tests for any changed behaviour in `core/`.
5. Update relevant docs under `docs/` if the public API or behaviour changes.

## Commit style

Use short, imperative subject lines:

```
fix: correct MurmurHash3 seed for 32-bit targets
feat: add Contains operator to targeting rules
docs: update browser SDK webpack config example
```

Prefix with `fix:`, `feat:`, `docs:`, `refactor:`, `test:`, or `ci:`.

## Running the test suite

```bash
# Rust unit + integration tests
cargo test --workspace

# Node.js SDK (requires a build first)
cd sdks/nodejs && npm install && npm run build:debug
```

> All four build scripts (`build`, `build:debug`, `build:zig`, `build:cross`) generate the
> platform-specific `checkgate.*.node` binary plus a `native-binding.js`/`native-binding.d.ts`
> dispatch loader — never `index.js`/`index.d.ts`. Those two are hand-written and wrap the native
> binding (see `sdks/nodejs/index.js`), which `require()`s `./native-binding.js`; they're not
> NAPI-RS auto-generated files, and no build script touches them.

## Submitting a PR

- Fill in the pull request template.
- Ensure CI passes (`cargo test`, clippy, fmt).
- Tag a maintainer for review if the change touches the evaluation engine or the SSE protocol.

## Reporting issues

Use the GitHub issue templates:
- **Bug report** — repro steps, environment, and error output.
- **Feature request** — motivation and proposed solution.

## License

By contributing you agree that your changes will be licensed under the
[Apache License 2.0](LICENSE), and that you have the right to grant that licence
for the work you submit.

## Refreshing dashboard screenshots

Run the disposable Docker demo, which uses separate container storage from your deployment:

```bash
docker compose -f docker-compose.screenshots.yml up --build -d
cd dashboard
bun install --frozen-lockfile
bunx playwright install chromium
bun run screenshots
```

The capture script creates the Vantage Robotics example workspace with administrator
Juan Dela Cruz, seeds flags and review requests, and replaces all eight images in
`assets/screenshots/`. The README and dashboard guide share those assets.
It expects the demo at `http://127.0.0.1:3100` and stops if an existing workspace does not
match the fixture. On Linux, Chromium may also need system libraries; install them with
`bunx playwright install-deps chromium` if Playwright reports missing dependencies.

After capturing, return to the repository root and remove the demo container:

```bash
cd ..
docker compose -f docker-compose.screenshots.yml down
```
