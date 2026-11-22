---
title: "Five-Minute Quickstart — Checkgate on Your Machine"
description: "Start Checkgate locally with one script, evaluate a flag from Node.js or Python, and watch a dashboard change reach your code. Development only."
---

# Five-minute quickstart

From a clone of the repository to a flag evaluated in your own process, and a dashboard change
reaching it live. Everything runs locally; the credentials are generated on your machine and are
for development only. For a deployment, follow [Getting Started](./getting-started) and
[Self-hosting](../self-hosting).

**You need:** Docker (Desktop or Engine, running) and Node.js 22 or later. For the Python example,
also Python 3.11 or newer and a Rust toolchain.

::: info Which version this runs
The script runs the latest published image (`ghcr.io/checkgate-sh/checkgate:latest`, currently
0.1.31). Everything below works with it except the **Playground** in step 3, which is new in 0.2.0.
To try 0.2.0 before it is published, build the image from this checkout first:
`docker compose build` at the repository root, then
`CHECKGATE_IMAGE=checkgate:latest ./quickstart/start.sh --seed`.
:::

## 1. Start Checkgate and seed a sample flag

```bash
git clone https://github.com/checkgate-sh/checkgate.git
cd checkgate
./quickstart/start.sh --seed
```

The script writes random secrets to `quickstart/.env`, starts the all-in-one image (server,
dashboard, PostgreSQL and Redis in one container) on port 3000, waits until it is ready, and
then completes the first-run setup with the one-time setup token, as the wizard would:

- an admin `admin@example.com` with a random password, printed once;
- the flag `new-checkout`, on for users whose `plan` is `pro`;
- the **Development** SDK key, written to `quickstart/.env` as `CHECKGATE_SDK_KEY`.

Prefer to click through the wizard yourself? Run `./quickstart/start.sh` without `--seed` and open
the printed setup URL with the printed token.

## 2. Evaluate the flag in your code

**Node.js**

```bash
cd quickstart/node
npm install
node --env-file=../.env index.mjs
```

**Python**

The Python package is not on PyPI yet, so build it from this checkout (needs Rust). Use a Python
3.11+ interpreter: if `python3 --version` prints an older one, write `python3.11` (or newer)
instead of `python3` below.

```bash
cd sdks/python
python3 -m venv .venv && .venv/bin/pip install "maturin>=1.9.4,<2"
VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release
cd ../../quickstart/python
set -a && . ../.env && set +a
../../sdks/python/.venv/bin/python main.py
```

Both print the flag for two users (`new-checkout: user-pro=true  user-free=false`; Python writes
`True`/`False`): the flag is evaluated in your process by the Checkgate engine, with no network
call per evaluation.

## 3. Change it live

Open [http://localhost:3000](http://localhost:3000), sign in with the printed credentials, and
switch `new-checkout` off. The running example prints the new answer within a second, without a
restart. With a 0.2.0 server <Since v="0.2.0" />, open **Playground** in the sidebar and evaluate
`new-checkout` for `{"plan": "pro"}` to see the matched rule and why.

<NextSteps :items="[
  { title: 'Core concepts', link: '/guide/concepts', text: 'Flags, rules, segments and rollouts.' },
  { title: 'Pick an SDK', link: '/sdks/', text: 'What your stack needs, and its status.' },
  { title: 'Deploy it', link: '/self-hosting', text: 'Docker, PostgreSQL, Redis and production settings.' },
]" />

## Clean up

```bash
./quickstart/start.sh --down    # stops the container and deletes its data and quickstart/.env
```

## Troubleshooting

| Symptom | Fix |
|---|---|
| `The Docker daemon is not running` | Start Docker Desktop (or `sudo systemctl start docker`) and run the script again. |
| `Port 3000 is already in use` | `CHECKGATE_PORT=3001 ./quickstart/start.sh --seed` (the examples read `CHECKGATE_PORT` from `.env`). |
| The dashboard is not reachable from another machine | By design: the quickstart listens on `127.0.0.1` only (it is plain HTTP for local development). `CHECKGATE_BIND=0.0.0.0 ./quickstart/start.sh` exposes it to your network; on Linux, Docker's port rules bypass ufw/firewalld. |
| `did not become ready within 2 minutes` | The script prints the container logs. A slow first image pull is the usual cause on a slow network; run it again. |
| Setup fails with `HTTP 409` or `403` | The instance was already set up. `./quickstart/start.sh --down` starts over. |
| The SDK logs `rejected the SDK key (HTTP 401)` | `CHECKGATE_SDK_KEY` is missing or from an older run: re-run with `--seed` after `--down`, or create a key in project settings. |
| The SDK keeps logging `reconnecting` | The server is not reachable at `http://localhost:$CHECKGATE_PORT`; check `docker compose -f quickstart/compose.yml ps`. |
| `pip install checkgate` finds nothing, or something unexpected | The Python package is **not on PyPI yet** — do not install a `checkgate` package from PyPI until the project announces it. Build it from the checkout: `cd sdks/python && python3 -m venv .venv && .venv/bin/pip install maturin && .venv/bin/maturin develop --release`, then run `main.py` with `sdks/python/.venv/bin/python`. |

## What the script does not do

It never weakens the server's setup protection: the setup token is required exactly as for the
wizard, and the server refuses to start without the generated `SESSION_SECRET`. The secrets
stay in `quickstart/.env`, which is ignored by Git. Do not reuse this file or `COOKIE_SECURE=false`
outside your machine.
