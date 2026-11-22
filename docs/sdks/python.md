---
title: "Python SDK — Local Feature Flag Evaluation for FastAPI, Django and Flask"
description: "Use the Checkgate Python SDK to evaluate feature flags in-process with the shared Rust engine (PyO3, abi3 wheels for CPython 3.11+). Synchronous, thread-safe evaluation, a background sync thread, offline cache, and guidance for gunicorn and uWSGI workers."
---

# Python SDK <Since v="0.2.0" />

<ReleaseNotice pkg="python" />

The Python SDK evaluates flags **in-process** with Checkgate's Rust engine
(the same core as every other SDK and the server), compiled into a CPython
extension module with PyO3. The flag set is kept current over Server-Sent
Events with a polling fallback, on one background thread that uses only the
standard library — there is no asyncio inside the client, so it never blocks
your event loop.

::: tip New in 0.2.0
The release workflow builds wheels for Linux x86_64 and aarch64 (manylinux), macOS x86_64 and
arm64, and Windows x86_64, for CPython 3.11 and newer (one abi3 wheel per platform). They are
not published anywhere yet. See [Platform support](#platform-support) for what is tested.
:::

## Installation

The package (`checkgate`, Python 3.11+) is **not on PyPI yet**: until the release announces it,
do not install a `checkgate` package from PyPI. Build it from a checkout of the repository. You need Rust and a Python 3.11+
interpreter; if `python3 --version` prints an older one, use `python3.11` (or newer) below.
Publishing to PyPI is planned; no release job publishes the wheels yet.

```bash
cd sdks/python
python3 -m venv .venv          # Python 3.11 or newer
.venv/bin/pip install "maturin>=1.9.4,<2"
VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release   # installs checkgate into .venv
```

Once it is published (not yet):

```bash
pip install checkgate
```

## Quick Start

```python
from checkgate import CheckgateClient, CheckgateTimeoutError

client = CheckgateClient("https://flags.yourcompany.com", sdk_key="your-sdk-key")
try:
    client.connect(timeout=10)   # waits for the full flag set
except CheckgateTimeoutError:
    pass                         # defaults until connected; keeps retrying in the background

if client.is_enabled("new-checkout", user.id, {"plan": user.plan}):
    ...

client.close()
```

In asyncio code, `await client.connect_async(timeout=10)` waits without
blocking the event loop. Evaluation itself is synchronous everywhere: it takes
microseconds and does no I/O, so call it directly from async handlers.

## API Reference

### `CheckgateClient(server_url, sdk_key=None, **options)`

| Option | Default | Description |
|---|---|---|
| `reconnect_delay_ms` | `3000` | Base stream reconnect delay; doubles per failed attempt |
| `max_reconnect_delay_ms` | `30000` | Cap of the reconnect delay (plus up to 30 % jitter) |
| `poll_fallback_threshold` | `3` | Failed stream attempts before also polling `/flags/snapshot`; `None` disables |
| `poll_interval_ms` | `30000` | Poll interval while polling |
| `connect_timeout_ms` | `30000` | A stream that sends no `connected` event in this time is abandoned and retried |
| `stream_idle_timeout_ms` | 3 × heartbeat | A connected stream silent this long is replaced; `0` disables |
| `request_timeout_ms` | `30000` | Socket timeout of a poll request |
| `storage` | `None` | Offline cache: `FileStorage(directory)`, `MemoryStorage()`, or any object with `get_item(key)` / `set_item(key, value)` |

### Evaluating flags

```python
client.is_enabled(flag_key, user_id, attributes=None) -> bool
client.get_value(flag_key, user_id, attributes=None, default=None)
client.get_variant(flag_key, user_id, attributes=None) -> Variant | None   # (enabled, value)
client.get_details(flag_key, user_id, attributes=None, default=None) -> EvaluationDetails
```

`attributes` maps names to **strings**; any other value is ignored (the same
rule as the Web, React Native and Flutter SDKs — the Node SDK raises
instead). Unknown flags return `False` / `default`.

`get_details()` explains an evaluation: `reason` (OpenFeature vocabulary:
`STATIC`, `DEFAULT`, `TARGETING_MATCH`, `SPLIT`, `DISABLED`,
`PREREQUISITE_FAILED`, or `ERROR` with `error_code` `FLAG_NOT_FOUND` /
`PROVIDER_NOT_READY`), the matched `rule_index`, the user's `rollout_bucket`,
the weighted `variant_index`, and each prerequisite's outcome.

### Lifecycle

| Method | |
|---|---|
| `connect(timeout=None)` | Blocks until the full flag set is applied. Raises `CheckgateAuthError` when the key is rejected (HTTP 401/403 — not retried; the last flags keep being served), `CheckgateTimeoutError` (also a `TimeoutError`), or `CheckgateClosedError`. |
| `await connect_async(timeout=None)` | The same for asyncio. |
| `close()` | Stops syncing; idempotent. Also usable as `with CheckgateClient(...) as client:`. Evaluations keep answering from the last flags held; `connect()` afterwards raises `CheckgateClosedError`. |
| `on_change(callback)` | Calls `callback(flag_key)` after a flag changes (not for the initial load). Returns an unsubscribe function. Runs on the sync thread. |

### Connection health

`client.get_status()` returns `{"state", "last_sync_at_ms", "error"}`, with
`state` one of `idle`, `connecting`, `streaming`, `reconnecting`, `polling`,
`auth_failed`, `closed`.

`client.get_health()` returns the SDK health object (schema 1) with
snake_case keys and millisecond timestamps: `state`, `ready`, `source`
(`none`, `offline_cache`, `server`), `flag_count`, `last_sync_at_ms`,
`config_age_ms`, `last_received_revision`, `last_applied_revision`,
`stale_events_ignored`, `reconnect_attempts`, `reconnect_count`, `polling`,
`last_poll_at_ms`, `last_error` (`{message, kind, at_ms}`) and
`offline_cache`. It never contains the SDK key, user ids or attributes, so it
is safe to expose on an internal health endpoint.

## Usage with FastAPI

```python
from contextlib import asynccontextmanager
from fastapi import FastAPI, Request
from checkgate import CheckgateClient, CheckgateTimeoutError

@asynccontextmanager
async def lifespan(app: FastAPI):
    client = CheckgateClient(CHECKGATE_URL, CHECKGATE_SDK_KEY)
    try:
        await client.connect_async(timeout=10)
    except CheckgateTimeoutError:
        pass
    app.state.flags = client
    yield
    client.close()

app = FastAPI(lifespan=lifespan)

@app.get("/checkout")
def checkout(request: Request, user_id: str):
    return {"v2": request.app.state.flags.is_enabled("new-checkout", user_id)}
```

## Usage with Django and Flask

Create the client once per process — Django in `AppConfig.ready()`, Flask in
the app factory or lazily on first use — and register `client.close` with
`atexit`. Complete examples are in the repository under `sdks/python/examples`.

## Workers, fork and preloading

Each worker process needs **its own client**. A client created before
`os.fork()` raises `CheckgateForkError` when used in the child: the child has
the client's memory but not its sync thread, and possibly a lock a parent
thread held at the moment of the fork.

- gunicorn without `--preload`, `uvicorn --workers`: create the client at app
  startup; every worker runs it.
- gunicorn with `--preload`: create the client in the `post_fork` hook, or
  lazily on first use.
- uWSGI: set `lazy-apps = true`, or create the client in a `@postfork`
  function.

## Resilience & Offline

The stream replay on each (re)connect is applied in one step, so an
evaluation never sees a half-updated flag set — a flag and its prerequisites
always come from the same version, even across threads. Late, duplicated or
reordered change events are ignored by revision; a silent stream is replaced
after three missed heartbeats; and after three failed reconnects the client
also polls the snapshot until the stream returns.

With `storage=FileStorage("/var/cache/myapp/checkgate")` the last flag set
is persisted and loaded on the next start, so a restart while the server is
unreachable serves the last known flags. Writes are atomic, so workers may
share the directory.

## Limitations

- No impression reporting or `track()` yet: the SDK makes no requests besides
  its stream and snapshot polls.
- `HTTP(S)_PROXY` environment variables are not used.
- CPython 3.11+ only; the abi3 wheel does not load on free-threaded builds.

## Platform support

| Platform | Wheel | Test suite runs on it |
|---|---|---|
| Linux x86_64 | Yes | Yes (Python 3.11 and 3.13) |
| Linux aarch64 | Yes (cross-compiled) | Yes, on a native build (Python 3.11) |
| macOS arm64 | Yes | Yes |
| macOS x86_64 | Yes (cross-compiled) | Yes, on a native build (Python 3.11) |
| Windows x86_64 | Yes | Yes (fork tests do not apply) |
