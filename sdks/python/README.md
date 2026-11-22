# checkgate (Python SDK)

Feature flags for Python services. Flags are evaluated locally by
[Checkgate](https://github.com/checkgate-sh/checkgate)'s Rust engine, the same
code the server and the other Checkgate SDKs run, and kept current over
Server-Sent Events with a polling fallback.

Current version: **0.2.0**, the first release of the Python SDK
([changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md)).

## What it does

- Evaluates flags in-process: an evaluation is a synchronous function call
  with no network request and no `await`, safe to call from any number of
  threads.
- Syncs on one background daemon thread that uses only the standard library
  (`http.client`, `threading`). There is no asyncio inside the client, so it
  does not depend on or block your event loop.
- Applies each full flag set as a whole and ignores late, duplicated or
  reordered change events by revision, so an evaluation never sees a
  half-applied update.
- Reconnects with exponential backoff, falls back to polling while the stream
  keeps failing, and can persist the last flag set for offline starts.
- Has no runtime dependencies besides its bundled extension module.

## Installation

```bash
pip install checkgate==0.2.0
```

Wheels are published for the platforms under "Platform compatibility" below;
0.2.0 is the package's first release. To build a wheel from source instead
(for example on a platform without a wheel), you need CPython 3.11 or newer
and a Rust toolchain (with rustup, the version pinned in the repository's
`rust-toolchain.toml` is installed automatically):

```bash
git clone --branch v0.2.0 https://github.com/checkgate-sh/checkgate.git
cd checkgate
python3 -m venv .venv
.venv/bin/pip install "maturin>=1.9.4,<2"
.venv/bin/maturin build --release --manifest-path sdks/python/Cargo.toml --out dist
.venv/bin/pip install dist/checkgate-0.2.0-*.whl
```

The last command installs into the clone's `.venv`; run the same
`pip install dist/...whl` with your application's interpreter to install it
there. On Windows, use `.venv\Scripts\` instead of `.venv/bin/`.

The release workflow builds wheels for the platforms listed under
"Platform compatibility" below and no source distribution,
so pip finds nothing to install on other platforms. Building from source there
is untested.

## Quick start

```python
from checkgate import CheckgateClient, CheckgateTimeoutError

client = CheckgateClient("https://flags.example.com", sdk_key="sk_live_your_key")
try:
    client.connect(timeout=10)  # waits for the full flag set
except CheckgateTimeoutError:
    pass  # evaluations return defaults; the client keeps connecting in the background

user_id = "user-123"
if client.is_enabled("new-checkout", user_id, {"plan": "pro"}):
    print("new checkout")

color = client.get_value("button-color", user_id, default="blue")
client.close()
```

In asyncio code, `await client.connect_async(timeout=10)` waits without
blocking the event loop. Evaluation stays synchronous everywhere; call it
directly from async handlers.

## Configuration

`CheckgateClient(server_url, sdk_key=None, *, ...)`. `server_url` must be an
`http://` or `https://` URL. All options are keyword-only:

| Option | Default | Description |
|---|---|---|
| `reconnect_delay_ms` | `3000` | Base stream reconnect delay; doubles per failed attempt, plus up to 30 % jitter |
| `max_reconnect_delay_ms` | `30000` | Cap of the reconnect delay (before jitter) |
| `poll_fallback_threshold` | `3` | Failed stream attempts before the client also polls `GET /flags/snapshot`; `None` disables polling |
| `poll_interval_ms` | `30000` | Poll interval while polling |
| `connect_timeout_ms` | `30000` | A stream that has not sent `connected` within this time is abandoned and retried |
| `stream_idle_timeout_ms` | `None` | A connected stream silent this long is replaced. `None`: 3 × the heartbeat interval the server announces (no idle timeout against servers older than 0.1.31, which announce none); `0` disables |
| `request_timeout_ms` | `30000` | Socket timeout of a poll request |
| `storage` | `None` | Offline cache: `FileStorage(directory)`, `MemoryStorage()`, or any object with `get_item(key)` and `set_item(key, value)` |

Invalid values raise `ValueError` (or `TypeError` for a `storage` without
those two methods) from the constructor.

## API overview

### Evaluation

| Method | Returns |
|---|---|
| `is_enabled(flag_key, user_id, attributes=None)` | `bool`; `False` for an unknown flag |
| `get_value(flag_key, user_id, attributes=None, default=None)` | the resolved value (`bool`, `str`, `int`, `float`, `dict`, `list` or `None`), or `default` for an unknown flag |
| `get_variant(flag_key, user_id, attributes=None)` | `Variant(enabled, value)`, or `None` for an unknown flag |
| `get_details(flag_key, user_id, attributes=None, default=None)` | `EvaluationDetails`: `flag_key`, `value`, `enabled`, `reason`, `checkgate_reason`, `rule_index`, `rollout_bucket`, `variant_index`, `prerequisites`, `error_code`, `error_message` |

`attributes` is a mapping of attribute names to strings. Values that are not
strings are skipped, because attribute rules compare strings (the Web, React
Native and Flutter SDKs do the same; the Node SDK raises instead). Pass
`str(value)` to target on a number.

`get_details().reason` uses the OpenFeature vocabulary: `STATIC`, `DEFAULT`,
`TARGETING_MATCH`, `SPLIT`, `DISABLED`, `PREREQUISITE_FAILED`, or `ERROR` with
`error_code` `FLAG_NOT_FOUND` or `PROVIDER_NOT_READY`. `checkgate_reason` is
the engine's own reason, which also distinguishes `ROLLOUT_EXCLUDED` (reported
as `SPLIT`).

### Lifecycle and status

| Method | Description |
|---|---|
| `connect(timeout=None)` | Blocks until the full flag set is applied (`timeout` in seconds; `None` waits indefinitely). Returns at once if already in sync |
| `await connect_async(timeout=None)` | The same for asyncio, without blocking the event loop or an executor thread |
| `close(timeout=2.0)` | Stops syncing and releases the connection; waits up to `timeout` seconds for the background threads. Idempotent |
| `on_change(callback)` | Calls `callback(flag_key)` after a flag is created, updated or deleted (not for the initial load). Returns an unsubscribe function |
| `is_ready()` | Whether a complete flag set from the server has been applied |
| `is_polling()` | Whether the poll fallback is active |
| `get_status()` | `{"state", "last_sync_at_ms", "error"}`; `state` is one of `idle`, `connecting`, `streaming`, `reconnecting`, `polling`, `auth_failed`, `closed` |
| `get_health()` | Sync diagnostics (schema 1): state, readiness, source (`none`, `offline_cache`, `server`), flag count, config age, revisions, stale events ignored, reconnects, polling, last error, offline cache |

The client is also a context manager: `with CheckgateClient(...) as client:`
and `async with CheckgateClient(...) as client:` close it on exit.

## Error handling

All exceptions derive from `CheckgateError`:

| Exception | Raised by | Meaning |
|---|---|---|
| `CheckgateAuthError` | `connect()`, `connect_async()` | The server rejected the SDK key (HTTP 401/403; `.status` holds the code). Not retried; flags already loaded keep being served. Call `connect()` again to retry |
| `CheckgateTimeoutError` | `connect()`, `connect_async()` | Not connected within `timeout`. Also a `TimeoutError`. The client keeps connecting in the background |
| `CheckgateClosedError` | `connect()`, `connect_async()` | The client was closed, before or while connecting |
| `CheckgateForkError` | evaluations, `connect()`, `connect_async()`, `on_change()`, `get_status()`, `get_health()` | The client was created before `os.fork()` and used in the child; see "Threads, workers and fork" below |

Evaluations do not raise for an unknown flag or a missing connection:

- Before the first flag set has arrived (and with no offline cache), every
  evaluation returns its default, `get_details()` reports `PROVIDER_NOT_READY`,
  and the client logs one warning.
- A flag this SDK version cannot parse is not served (it evaluates as
  unknown) and a warning is logged.
- Network errors and unusable responses are retried, logged on the
  `checkgate` logger and recorded in `get_health()["last_error"]`. An
  exception raised by an `on_change` callback is logged. None of these reach
  the caller.

## Threads, workers and fork

Evaluations run on the caller's thread and never wait for the sync thread.
`on_change` callbacks run on the sync thread: keep them short, and do not call
`connect()` from one (it raises `RuntimeError`).

`close()` is terminal for syncing: `connect()` afterwards raises
`CheckgateClosedError`, and `on_change` registers nothing. Evaluations keep
answering from the last flags held, so requests still in flight during
shutdown are not disrupted, but the flags no longer change.

Create the client **in the process that uses it**. A client created before
`os.fork()` raises `CheckgateForkError` when used in the child: the child
inherits the client's memory but not its sync thread, and possibly a lock a
parent thread held at the moment of the fork, which would deadlock it. The
client detects the fork (`os.register_at_fork`) and refuses instead of
serving flags that never update.

- **gunicorn without `--preload`, `uvicorn --workers`, Django `runserver`**:
  each worker imports the app, so creating the client at app startup works
  (one stream per worker).
- **gunicorn `--preload` / `preload_app = True`**: the app is imported in the
  master before the fork. Create the client in gunicorn's `post_fork` hook, or
  lazily on first use (as the Flask example below does), and do not also
  create it in code that runs in the master.
- **uWSGI**: the default `lazy-apps = false` loads the app in the master and
  forks. Set `lazy-apps = true`, or create the client in a `@postfork`
  function.
- **`multiprocessing` with the `fork` start method**: create the client in
  the worker function.

Each worker process holds its own stream connection and flag set.

## Framework examples

Complete versions are in
[`sdks/python/examples`](https://github.com/checkgate-sh/checkgate/tree/main/sdks/python/examples).
They read `CHECKGATE_URL` and `CHECKGATE_SDK_KEY` from the environment
(Django from settings).

### FastAPI

The lifespan runs in every uvicorn worker, so each worker gets its own client.

```python
import os
from contextlib import asynccontextmanager

from fastapi import FastAPI, Request

from checkgate import CheckgateClient, CheckgateTimeoutError


@asynccontextmanager
async def lifespan(app: FastAPI):
    client = CheckgateClient(os.environ["CHECKGATE_URL"], os.environ["CHECKGATE_SDK_KEY"])
    try:
        await client.connect_async(timeout=10)
    except CheckgateTimeoutError:
        pass  # start anyway: defaults until the flags arrive
    app.state.flags = client
    yield
    client.close()


app = FastAPI(lifespan=lifespan)


@app.get("/checkout")
def checkout(request: Request, user_id: str, plan: str = "free"):
    flags: CheckgateClient = request.app.state.flags
    return {"checkout": "v2" if flags.is_enabled("new-checkout", user_id, {"plan": plan}) else "v1"}
```

### Django

Create the client in `AppConfig.ready()` and close it at exit:

```python
# myapp/flags.py
import atexit
from typing import Optional

from django.conf import settings

from checkgate import CheckgateClient, CheckgateTimeoutError

client: Optional[CheckgateClient] = None


def start() -> CheckgateClient:
    global client
    if client is None:
        client = CheckgateClient(settings.CHECKGATE_URL, settings.CHECKGATE_SDK_KEY)
        try:
            client.connect(timeout=5)
        except CheckgateTimeoutError:
            pass
        atexit.register(client.close)
    return client


# myapp/apps.py
from django.apps import AppConfig


class MyAppConfig(AppConfig):
    name = "myapp"

    def ready(self) -> None:
        from . import flags

        flags.start()
```

In views, use `flags.client.is_enabled(...)`. With gunicorn `--preload` or
uWSGI's default `lazy-apps = false`, `ready()` runs in the master: call
`flags.start()` from a post-fork hook instead of from `ready()`.

### Flask

Flask's `teardown_appcontext` runs after every request, so close the client at
process exit. Creating it lazily on first use works with or without gunicorn
`--preload`:

```python
import atexit
import os
import threading

from flask import Flask, current_app, request

from checkgate import CheckgateClient, CheckgateTimeoutError

_lock = threading.Lock()


def get_flags() -> CheckgateClient:
    """The process's client, created on first use (after any fork)."""
    client = current_app.extensions.get("checkgate")
    if client is None:
        with _lock:
            client = current_app.extensions.get("checkgate")
            if client is None:
                config = current_app.config
                client = CheckgateClient(config["CHECKGATE_URL"], config["CHECKGATE_SDK_KEY"])
                try:
                    client.connect(timeout=5)
                except CheckgateTimeoutError:
                    pass
                atexit.register(client.close)
                current_app.extensions["checkgate"] = client
    return client


def create_app() -> Flask:
    app = Flask(__name__)
    app.config["CHECKGATE_URL"] = os.environ["CHECKGATE_URL"]
    app.config["CHECKGATE_SDK_KEY"] = os.environ["CHECKGATE_SDK_KEY"]

    @app.get("/")
    def index():
        enabled = get_flags().is_enabled("new-homepage", request.args.get("user", "anonymous"))
        return {"homepage": "new" if enabled else "classic"}

    return app
```

## Offline cache

```python
from checkgate import CheckgateClient, FileStorage

client = CheckgateClient(
    "https://flags.example.com",
    sdk_key="sk_live_your_key",
    storage=FileStorage("/var/cache/myapp/checkgate"),
)
```

The last complete flag set is saved after every change and loaded on the next
`connect()`, before the stream opens, so a restart while the server is
unreachable serves the last known flags instead of defaults
(`get_health()["source"] == "offline_cache"`). Writes are atomic renames, so
several workers may share one directory (the last writer wins). Storage errors
are logged and ignored.

## Platform compatibility

CPython 3.11 or newer, one abi3 wheel per platform. The abi3 wheel does not
load on free-threaded builds (`3.13t`, `3.14t`); other interpreters such as
PyPy are not supported.

| Platform | Release wheel | Tested on that platform |
|---|---|---|
| Linux x86_64 (manylinux) | built on x86_64 | the release wheel (3.11); CI and python-wheels workflow (3.11, 3.13) |
| Linux aarch64 (manylinux) | cross-built on x86_64 | python-wheels workflow on an arm64 runner (3.11), including a wheel built the way the release builds it; the release wheel itself is not run |
| macOS arm64 | built on arm64 | the release wheel (3.11); python-wheels workflow (3.11, 3.13) |
| macOS x86_64 | cross-built on arm64 | python-wheels workflow on an Intel runner (3.11), with a natively built wheel; the release wheel itself is not run |
| Windows x86_64 | built on x64 | the release wheel (3.11); python-wheels workflow (3.11). The fork tests are skipped |

The release workflow runs the test suite against the wheels it publishes on
Linux x86_64, macOS arm64 and Windows x86_64. The python-wheels workflow
builds and tests wheels natively on every listed platform for pull requests
that change the engine or this SDK. There are no wheels for musllinux
(Alpine), 32-bit platforms or Windows on Arm.

## Limitations

- No impression reporting and no `track()`: the client sends nothing but its
  stream and poll requests. Experiment analytics need another SDK or the HTTP
  API.
- No HTTP proxy support: `HTTP_PROXY` / `HTTPS_PROXY` are not read
  (`http.client` does not support them).

## Security considerations

- Use an `https://` server URL in production: the SDK key is sent as an
  `Authorization: Bearer` header on every stream and poll request. TLS
  certificates and host names are verified with Python's default SSL context
  (the system trust store).
- An SDK key only reaches the SDK endpoints of its environment. Keep it in
  configuration or a secret store, not in source code.
- User ids and attributes stay in the process: nothing about an evaluation is
  sent to the server.
- `get_health()` and `get_status()` contain no SDK key, user id or attribute;
  `CheckgateAuthError` carries only the HTTP status. `get_health()` is
  suitable for an internal health endpoint.
- `FileStorage` holds flag configuration only. Files are named by a hash (no
  server URL or key in the name) and created readable by the owner only.

## Documentation

- Python SDK guide: https://checkgate-sh.github.io/checkgate/sdks/python
- All SDKs: https://checkgate-sh.github.io/checkgate/sdks/
- Changelog: https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md
- Source: https://github.com/checkgate-sh/checkgate/tree/main/sdks/python

## Development

From `sdks/python` in a clone of the repository (Rust toolchain and Node.js
needed):

```bash
python3 -m venv .venv
.venv/bin/pip install "maturin>=1.9.4,<2" pytest
VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release
.venv/bin/python -m pytest tests -q
node --test --test-concurrency=1 ../conformance/python.mjs   # SSE fault-injection suite
```

`src/lib.rs` is the extension module (`checkgate._core`, evaluation only);
`python/checkgate/` is the client.

## License

Apache-2.0
