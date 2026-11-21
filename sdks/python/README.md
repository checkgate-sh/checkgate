# Checkgate Python SDK

Feature flags for Python services, evaluated **locally** by Checkgate's Rust
engine (the same code the server and every other Checkgate SDK run) and kept
current over Server-Sent Events, with a polling fallback.

- Evaluation is a synchronous, in-process call: no network round trip and no
  `await`. It is safe to call from any number of threads.
- Sync runs on one background daemon thread using only the standard library
  (`http.client`, `threading`): no asyncio inside the client, so it never
  blocks or depends on your event loop.
- CPython 3.11+ (abi3 wheels). The only runtime dependency is the bundled
  extension module.

> Status: new in 0.2.0. Wheels are built for Linux x86_64/aarch64 (manylinux),
> macOS x86_64/arm64 and Windows x86_64; see [Platform support](#platform-support)
> for which of them are tested.

## Install

```bash
# Not on PyPI yet — do not install a `checkgate` package from PyPI until the
# project announces it. Build from a checkout instead (see Development below).
pip install checkgate
```

## Quick start

```python
from checkgate import CheckgateClient, CheckgateTimeoutError

client = CheckgateClient("https://flags.example.com", sdk_key="sk_live_your_key")
try:
    client.connect(timeout=10)          # waits for the full flag set
except CheckgateTimeoutError:
    pass                                # serve defaults; keeps connecting in the background

if client.is_enabled("new-checkout", user_id, {"plan": "pro"}):
    ...

color = client.get_value("button-color", user_id, default="blue")
client.close()
```

asyncio code waits without blocking the loop:

```python
await client.connect_async(timeout=10)
```

Framework examples: [`examples/`](examples) (FastAPI lifespan, Flask app
factory, Django `AppConfig.ready()`, gunicorn `post_fork`).

## API

### `CheckgateClient(server_url, sdk_key=None, *, ...)`

| Option | Default | |
|---|---|---|
| `reconnect_delay_ms` | 3000 | base stream reconnect delay; doubles per failed attempt |
| `max_reconnect_delay_ms` | 30000 | cap of the delay (plus 0–30 % jitter) |
| `poll_fallback_threshold` | 3 | failed stream attempts before also polling `GET /flags/snapshot`; `None` disables |
| `poll_interval_ms` | 30000 | poll interval while polling |
| `connect_timeout_ms` | 30000 | a stream that has not sent `connected` within this is abandoned and retried |
| `stream_idle_timeout_ms` | 3 × server heartbeat | a silent connected stream is treated as dead; `0` disables; none against servers before 0.1.31 |
| `request_timeout_ms` | 30000 | socket timeout of a poll request |
| `storage` | `None` | offline persistence (`FileStorage(dir)`, `MemoryStorage()`, or any object with `get_item`/`set_item`) |

### Evaluation

| Method | Returns |
|---|---|
| `is_enabled(flag, user_id, attributes=None)` | `bool`; `False` for an unknown flag |
| `get_value(flag, user_id, attributes=None, default=None)` | the resolved value (`bool`, `str`, `int`, `float`, `dict`, `list`, `None`), or `default` for an unknown flag |
| `get_variant(flag, user_id, attributes=None)` | `Variant(enabled, value)` or `None` |
| `get_details(flag, user_id, attributes=None, default=None)` | `EvaluationDetails`: value, `reason`, `rule_index`, `rollout_bucket`, `variant_index`, prerequisite trace, `error_code` |

`attributes` is a mapping of strings. **Values that are not strings are
skipped** (attribute rules compare strings), exactly as the web, React
Native and Flutter SDKs do; the Node SDK raises instead. Pass
`str(value)` to target on a number.

Before the first flag set has arrived (and with no offline cache) every
evaluation returns its default, and `get_details` reports `PROVIDER_NOT_READY`.

`get_details().reason` follows OpenFeature: `STATIC`, `DEFAULT`,
`TARGETING_MATCH`, `SPLIT`, `DISABLED`, `PREREQUISITE_FAILED`, or `ERROR`
(with `error_code` `FLAG_NOT_FOUND` or `PROVIDER_NOT_READY`).
`checkgate_reason` keeps the engine's own reason, which also distinguishes
`ROLLOUT_EXCLUDED` (mapped to `SPLIT`).

### Lifecycle and status

| | |
|---|---|
| `connect(timeout=None)` | blocks until the full flag set is applied. Raises `CheckgateAuthError` (key rejected, HTTP 401/403: not retried), `CheckgateTimeoutError` (also a `TimeoutError`; keeps connecting in the background) or `CheckgateClosedError` |
| `await connect_async(timeout=None)` | the same, without blocking the event loop or an executor thread |
| `close(timeout=2.0)` | stops syncing and releases the connection; idempotent. Also `with CheckgateClient(...) as client:` |
| `on_change(callback) -> unsubscribe` | `callback(flag_key)` after a flag is created, updated or deleted (not for the initial bootstrap). Runs on the sync thread: keep it short, do not call `connect()` from it |
| `get_status()` | `{"state", "last_sync_at_ms", "error"}`; `state` is `idle`, `connecting`, `streaming`, `reconnecting`, `polling`, `auth_failed` or `closed` |
| `get_health()` | sync diagnostics (schema 1): state, readiness, source (`none`/`offline_cache`/`server`), flag count, config age, revisions, stale events ignored, reconnects, polling, last error, offline cache. Contains no SDK key, user id or attribute |

After `close()`: `connect()` raises `CheckgateClosedError`; evaluations keep
answering from the last flags held (requests still in flight during shutdown
are not disrupted) but never change again; `on_change` registers nothing.

## How syncing works

The client opens `GET /stream`. On every (re)connect the server replays the
full flag set and sends `ready`; the replay is staged and swapped in whole,
so evaluations never see a half-applied set. Afterwards each change arrives as
a revisioned delta; an event that is not newer than what the client holds
(late, duplicated, reordered) is ignored. A stream that goes silent for 3
heartbeats is replaced; reconnects back off exponentially with jitter, and
after 3 failed attempts the client also polls `/flags/snapshot?v=2`, merging
per flag by revision so a delayed replay cannot roll back newer polled state.
HTTP 401/403 stops all retries (`auth_failed`) and keeps the last flags.

The flag set is swapped atomically in the engine: a flag and its
prerequisites always come from the same version, even while another thread
applies an update.

## Workers, `fork()` and preloading

A client must be created **in the process that uses it**. A client created
before `os.fork()` raises `CheckgateForkError` when used in the child: the
child inherits the client's memory but not its sync thread, and possibly a
lock that some parent thread held at the instant of the fork (which would
deadlock the child). The client detects the fork (`os.register_at_fork`) and
refuses instead of silently serving flags that never update.

- **gunicorn without `--preload`, uvicorn `--workers`, Django runserver**: each
  worker imports your app, so creating the client at startup just works (one
  stream per worker).
- **gunicorn `--preload` / `preload_app = True`**: create the client in
  `post_fork` (see [`examples/gunicorn.conf.py`](examples/gunicorn.conf.py)) or
  lazily on first use (see [`examples/flask_app.py`](examples/flask_app.py)).
- **uWSGI**: its default (`lazy-apps = false`) loads the app in the master
  and forks; set `lazy-apps = true` or create the client in a
  `@postfork` function.
- **multiprocessing with the `fork` start method**: create the client in the
  worker function.

Each worker holds its own stream connection and flag set. Point several
workers at one `FileStorage` directory if you like: writes are atomic renames
(last writer wins).

## Offline cache

```python
from checkgate import CheckgateClient, FileStorage

client = CheckgateClient(url, key, storage=FileStorage("/var/cache/myapp/checkgate"))
```

The last complete flag set is saved after every change and loaded on the next
start, before connecting, so a restart while the server is unreachable serves
the last known flags instead of defaults (`get_health()["source"] ==
"offline_cache"`). Files are named by a hash and hold flag configuration only.

## Limitations

- **No impression reporting or `track()`** in this release: the Python SDK
  sends nothing but its stream and poll requests. Experiment analytics need
  another SDK or the HTTP API.
- **No HTTP proxy support** (`HTTP(S)_PROXY` is not read: `http.client` does
  not support it). TLS uses the system's default trust store.
- **CPython only**, 3.11 or newer. The abi3 wheel does not load on
  free-threaded (`3.13t`) builds.
- Evaluations are synchronous and take microseconds; the GIL is released
  inside the engine, but the attribute conversion runs with it held.

## Platform support

| Platform | Wheel built by | Test suite run by |
|---|---|---|
| Linux x86_64 (manylinux) | release workflow, CI | CI `sdk-python` (Python 3.11 and 3.13), release workflow |
| Linux aarch64 (manylinux) | release workflow (cross) | nothing yet |
| macOS arm64 | release workflow | release workflow |
| macOS x86_64 | release workflow (cross) | by hand (Python 3.11 and 3.13) |
| Windows x86_64 | release workflow | release workflow (fork tests skip) |

The workflows are new with this SDK: until they have run, only the macOS
x86_64 wheel has been built and tested.

## Development

```bash
cd sdks/python
python3.11 -m venv .venv
.venv/bin/pip install "maturin>=1.9.4,<2" pytest
VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release
.venv/bin/python -m pytest tests -q
node --test --test-concurrency=1 ../conformance/python.mjs   # SSE fault-injection suite
.venv/bin/python tool/bench.py                                # evaluation latency
```

`src/lib.rs` is the extension module (`checkgate._core`, evaluation only);
`python/checkgate/` is the client.
