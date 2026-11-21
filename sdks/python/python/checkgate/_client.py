"""The Checkgate client: local flag evaluation, kept in sync with the server.

The sync state machine is a port of ``sdks/nodejs/index.js`` (the reference
client): SSE stream with staged replay swapped in on ``ready``, per-flag
revision rules with the learn counter (R20), poll fallback, exponential
backoff with jitter, connect deadline, idle watchdog, terminal auth failure
and offline persistence. Where this file differs from it, a comment says so.

Threads. One daemon thread ("checkgate-sync") runs every state change, one
task at a time, like the Node event loop. Each stream attempt and each poll
does its blocking I/O on its own short-lived daemon thread and posts what it
read to the sync thread. Evaluations run on the caller's thread and never
wait for the sync thread: they read the Rust engine, which swaps flag sets
atomically.

Locks. ``_lock`` guards all sync state; the sync thread holds it for each
task, public methods for their reads and writes. No user code (change
listeners, storage adapters) runs while it is held.
"""

from __future__ import annotations

import asyncio
import json
import logging
import math
import os
import random
import threading
import time
from dataclasses import dataclass, field
from typing import Any, Callable, Literal, Mapping, NamedTuple, Optional, TypedDict, cast

from . import _core
from ._errors import (
    CheckgateAuthError,
    CheckgateClosedError,
    CheckgateForkError,
    CheckgateTimeoutError,
)
from ._loop import Loop, Timer
from ._revisions import (
    Rev,
    canonical,
    event_revision,
    is_stale_event,
    loads,
    merge_full_set,
    parse_snapshot,
)
from ._storage import Storage, cache_namespace
from ._transport import Endpoint, PollRequest, Request, StreamRequest

__version__ = "0.1.31"

log = logging.getLogger("checkgate")

# Longest accepted delay or timeout, in ms (Node's timer ceiling, 2^31 - 1;
# about 24.8 days). Python's timers fail beyond threading.TIMEOUT_MAX.
MAX_DELAY_MS = 2**31 - 1

#: HTTP statuses that mean the SDK key itself is rejected: retrying cannot help.
AUTH_FAILURE_STATUSES = frozenset({401, 403})

#: A healthy stream is never silent for longer than the server's heartbeat
#: interval; it is declared dead after this many intervals without an event.
IDLE_HEARTBEATS = 3

#: Stream events that prove a stream alive (what the Node client listens to).
_KNOWN_EVENTS = frozenset({"connected", "heartbeat", "ready", "update"})

State = Literal["idle", "connecting", "streaming", "reconnecting", "polling", "auth_failed", "closed"]

#: OpenFeature resolution reasons for the core's reasons (CONTRACTS: the OF
#: provider mapping). ROLLOUT_EXCLUDED is a hash-decided outcome: SPLIT.
OPENFEATURE_REASONS = {
    "DISABLED": "DISABLED",
    "PREREQUISITE_FAILED": "PREREQUISITE_FAILED",
    "TARGETING_MATCH": "TARGETING_MATCH",
    "ROLLOUT_EXCLUDED": "SPLIT",
    "SPLIT": "SPLIT",
    "DEFAULT": "DEFAULT",
    "STATIC": "STATIC",
}


# ---------------------------------------------------------------------------
# Fork detection: a client must not be used in a child created by os.fork()
# (gunicorn/uwsgi preload). Its sync thread did not survive the fork, and a
# lock some parent thread held at that instant (the state lock, the engine's)
# stays locked forever in the child. Every public method compares the epoch
# it was created in with the current one: one attribute read.
# ---------------------------------------------------------------------------


class _ForkEpoch:
    value = 0


def _after_fork_in_child() -> None:
    _ForkEpoch.value += 1


if hasattr(os, "register_at_fork"):
    os.register_at_fork(after_in_child=_after_fork_in_child)


def _now_ms() -> int:
    return int(time.time() * 1000)


def idle_timeout_for(heartbeat_ms: Any, override: Optional[float]) -> float:
    """How long a connected stream may stay silent before it is considered
    dead: an explicit ``stream_idle_timeout_ms`` (0 disables), else
    IDLE_HEARTBEATS times the heartbeat interval the server announced.
    Servers before 0.1.31 announce none and send no heartbeat events, so a
    quiet stream is not timed out."""
    if override is not None and override >= 0:
        return float(override)
    if isinstance(heartbeat_ms, (int, float)) and not isinstance(heartbeat_ms, bool) and heartbeat_ms > 0:
        return float(heartbeat_ms) * IDLE_HEARTBEATS
    return 0.0


class Variant(NamedTuple):
    """A flag's resolved state: on/off, and its value (bool, str, int, float,
    dict, list or None)."""

    enabled: bool
    value: Any


@dataclass(frozen=True)
class EvaluationDetails:
    """An evaluation with the reason behind it (OpenFeature-aligned).

    ``reason`` is the OpenFeature reason (``STATIC``, ``DEFAULT``,
    ``TARGETING_MATCH``, ``SPLIT``, ``DISABLED``, ``PREREQUISITE_FAILED``, or
    ``ERROR``); ``checkgate_reason`` the engine's own (it also distinguishes
    ``ROLLOUT_EXCLUDED`` from ``SPLIT``). On ``ERROR``, ``error_code`` is
    ``FLAG_NOT_FOUND`` or ``PROVIDER_NOT_READY`` and ``value`` is the default.
    """

    flag_key: str
    value: Any
    enabled: bool
    reason: str
    checkgate_reason: Optional[str] = None
    variant_index: Optional[int] = None
    rule_index: Optional[int] = None
    rollout_bucket: Optional[int] = None
    prerequisites: tuple[dict[str, Any], ...] = ()
    error_code: Optional[str] = None
    error_message: Optional[str] = None


class Status(TypedDict):
    state: State
    last_sync_at_ms: Optional[int]
    error: Optional[BaseException]


ErrorKind = Literal["auth", "network", "http", "parse", "timeout"]


class LastError(TypedDict):
    message: str
    kind: ErrorKind
    at_ms: int


class OfflineCacheHealth(TypedDict):
    configured: bool
    hydrated: bool
    last_persist_at_ms: Optional[int]


class Health(TypedDict):
    schema_version: int
    state: State
    ready: bool
    source: Literal["none", "offline_cache", "server"]
    flag_count: int
    last_sync_at_ms: Optional[int]
    config_age_ms: Optional[int]
    last_received_revision: Optional[int]
    last_applied_revision: Optional[int]
    stale_events_ignored: int
    reconnect_attempts: int
    reconnect_count: int
    polling: bool
    last_poll_at_ms: Optional[int]
    last_error: Optional[LastError]
    offline_cache: OfflineCacheHealth


@dataclass
class _Attempt:
    """One pending connect(): settled by "ready" (or a poll), an auth
    failure, or close()."""

    event: threading.Event = field(default_factory=threading.Event)
    error: Optional[BaseException] = None
    waiters: list[tuple[asyncio.AbstractEventLoop, asyncio.Future[None]]] = field(default_factory=list)


def _wake(fut: asyncio.Future[None], error: Optional[BaseException]) -> None:
    if fut.done():
        return
    if error is None:
        fut.set_result(None)
    else:
        fut.set_exception(error)


class CheckgateClient:
    """Evaluates feature flags locally, kept current by the Checkgate server.

    ::

        client = CheckgateClient("https://flags.example.com", sdk_key="sk_live_...")
        client.connect(timeout=10)
        if client.is_enabled("new-checkout", user_id, {"plan": "pro"}):
            ...
        client.close()

    Evaluations are synchronous and local (no network I/O) and safe from any
    number of threads. Syncing runs on a background daemon thread.

    Do not use a client across ``os.fork()``: create it in each worker
    process (see ``CheckgateForkError``).

    Args:
        server_url: Base URL of the Checkgate server.
        sdk_key: SDK key of the environment.
        reconnect_delay_ms: Base stream reconnect delay; grows exponentially.
        max_reconnect_delay_ms: Cap of the reconnect delay (before jitter).
        poll_fallback_threshold: Consecutive failed stream attempts before
            also polling ``GET /flags/snapshot``. ``None`` or ``math.inf``
            disables polling.
        poll_interval_ms: Poll interval while polling.
        connect_timeout_ms: A stream attempt that has not received the
            server's ``connected`` event in this time is abandoned and retried.
        stream_idle_timeout_ms: A connected stream silent (no event, heartbeats
            included) this long is treated as dead. Default: 3 x the heartbeat
            interval the server announces; no idle timeout against servers
            older than 0.1.31. ``0`` disables it.
        request_timeout_ms: Socket timeout of a poll request.
        storage: Optional persistence adapter (``get_item``/``set_item``, e.g.
            ``FileStorage``): the last flag set is saved and loaded on the next
            start, so flags evaluate before or without a connection.
    """

    def __init__(
        self,
        server_url: str,
        sdk_key: Optional[str] = None,
        *,
        reconnect_delay_ms: float = 3000,
        max_reconnect_delay_ms: float = 30000,
        poll_fallback_threshold: Optional[float] = 3,
        poll_interval_ms: float = 30000,
        connect_timeout_ms: float = 30000,
        stream_idle_timeout_ms: Optional[float] = None,
        request_timeout_ms: float = 30000,
        storage: Optional[Storage] = None,
    ) -> None:
        for name, value in (
            ("reconnect_delay_ms", reconnect_delay_ms),
            ("max_reconnect_delay_ms", max_reconnect_delay_ms),
            ("poll_interval_ms", poll_interval_ms),
            ("connect_timeout_ms", connect_timeout_ms),
            ("request_timeout_ms", request_timeout_ms),
        ):
            if (not isinstance(value, (int, float)) or isinstance(value, bool) or not value > 0
                    or not math.isfinite(value) or value > MAX_DELAY_MS):
                raise ValueError(f"{name} must be a positive number of milliseconds, at most {MAX_DELAY_MS}")
        if stream_idle_timeout_ms is not None and (
                not isinstance(stream_idle_timeout_ms, (int, float)) or isinstance(stream_idle_timeout_ms, bool)
                or not 0 <= stream_idle_timeout_ms <= MAX_DELAY_MS):
            # Infinity used to pass and then stop the sync thread for good
            # (a timer beyond threading.TIMEOUT_MAX): 0 disables the timeout.
            raise ValueError(f"stream_idle_timeout_ms must be 0 (disabled) to {MAX_DELAY_MS}, or None")
        if storage is not None and not isinstance(storage, Storage):
            raise TypeError("storage must have get_item(key) and set_item(key, value)")

        self._endpoint = Endpoint(server_url)
        self.server_url = server_url.rstrip("/")
        self._headers = {"User-Agent": f"checkgate-python/{__version__}"}
        if sdk_key:
            self._headers["Authorization"] = f"Bearer {sdk_key}"
        self.reconnect_delay_ms = float(reconnect_delay_ms)
        self.max_reconnect_delay_ms = float(max_reconnect_delay_ms)
        self.poll_fallback_threshold = math.inf if poll_fallback_threshold is None else poll_fallback_threshold
        self.poll_interval_ms = float(poll_interval_ms)
        self.connect_timeout_ms = float(connect_timeout_ms)
        self.stream_idle_timeout_ms = stream_idle_timeout_ms
        self.request_timeout_ms = float(request_timeout_ms)

        self._epoch = _ForkEpoch.value
        self._engine = _core.Engine()
        self._lock = threading.Lock()
        self._loop = Loop("checkgate-sync")
        self._workers: set[threading.Thread] = set()

        self._ready = False
        self._hydrated = False
        self._hydrated_from_cache = False
        self._closed = False
        self._user_closed = False
        self._state: State = "idle"
        self._attempt: Optional[_Attempt] = None
        self._listeners: tuple[Callable[[str], None], ...] = ()
        # Work done after a task, outside the lock: change notifications
        # and the offline snapshot write.
        self._pending_changes: list[str] = []
        self._persist_pending = False
        self._warned_not_ready = False

        # The raw flag set the server sent (key -> flag JSON), for diffing,
        # persistence and conformance checks. The engine holds the subset
        # that parsed; see _apply_engine.
        self._flag_cache: dict[str, Any] = {}
        # During a (re)connect replay: the flags received so far and their
        # revisions. Swapped in as a whole on "ready".
        self._staged: Optional[dict[str, Any]] = None
        self._staged_revisions: Optional[dict[str, Rev]] = None
        # Last applied revision per flag key, with `at` = the learn counter
        # when it was applied. The counter grows with every applied change; a
        # full set compares it with the value at its request. Never a clock.
        self._revisions: dict[str, Rev] = {}
        self._learn_seq = 0
        self._stream_started_at = 0
        self._env_id: Optional[str] = None

        self._stream: Optional[StreamRequest] = None
        self._idle_timeout_ms = 0.0
        self._watchdog: Optional[Timer] = None
        self._watchdog_fire_at = 0.0
        self._watchdog_deadline: Optional[float] = None

        self._reconnect_attempts = 0
        self._reconnect_timer: Optional[Timer] = None

        self._polling = False
        self._poll_timer: Optional[Timer] = None
        self._sync_generation = 0
        self._poll_request: Optional[PollRequest] = None

        self.storage = storage
        self._cache_key = f"checkgate:flags:v2:{self.server_url}:{cache_namespace(sdk_key or 'session')}"

        # Health (CONTRACTS.md, schema 1).
        self._last_sync_at: Optional[int] = None
        self._status_error: Optional[BaseException] = None
        self._health_error: Optional[LastError] = None
        self._source: Literal["none", "offline_cache", "server"] = "none"
        self._last_received_revision: Optional[float] = None
        self._stale_events_ignored = 0
        self._reconnect_count = 0
        self._last_poll_at: Optional[int] = None
        self._last_persist_at: Optional[int] = None

    # ------------------------------------------------------------------
    # Lifecycle
    # ------------------------------------------------------------------

    def connect(self, timeout: Optional[float] = None) -> None:
        """Connects to the server's stream and waits for the full flag set.

        Returns once the server has replayed every flag (or a poll delivered
        them). Returns at once if the client is already in sync. Starts
        automatic reconnection on connection loss.

        Args:
            timeout: Seconds to wait; None waits until connected.

        Raises:
            CheckgateAuthError: the server rejected the SDK key (HTTP 401/403).
                Not retried; flags already loaded keep being served.
            CheckgateTimeoutError: not connected within ``timeout``. The
                client keeps connecting in the background.
            CheckgateClosedError: the client was closed.
        """
        self._check_fork()
        if self._loop.in_loop():
            raise RuntimeError("connect() cannot wait inside a Checkgate change listener")
        attempt = self._begin_connect()
        if attempt is None:
            return
        if not attempt.event.wait(timeout):
            raise CheckgateTimeoutError(
                f"[Checkgate] Not connected within {timeout} s; still connecting in the background.")
        if attempt.error is not None:
            raise attempt.error

    async def connect_async(self, timeout: Optional[float] = None) -> None:
        """``connect()`` for asyncio code: waits without blocking the event
        loop (and without occupying an executor thread). Same errors."""
        self._check_fork()
        attempt = self._begin_connect()
        if attempt is None:
            return
        loop = asyncio.get_running_loop()
        fut: asyncio.Future[None] = loop.create_future()
        with self._lock:
            if attempt.event.is_set():
                _wake(fut, attempt.error)
            else:
                attempt.waiters.append((loop, fut))
        try:
            await asyncio.wait_for(fut, timeout)
        except asyncio.TimeoutError:
            raise CheckgateTimeoutError(
                f"[Checkgate] Not connected within {timeout} s; still connecting in the background."
            ) from None

    def _begin_connect(self) -> Optional[_Attempt]:
        with self._lock:
            if self._closed:
                raise CheckgateClosedError("[Checkgate] The client is closed; create a new one.")
            if self._ready and self._state != "auth_failed":
                return None
            if self._attempt is not None:
                return self._attempt
            self._user_closed = False
            self._status_error = None
            self._state = "reconnecting" if self._ready else "connecting"
            attempt = self._attempt = _Attempt()
        self._loop.start()
        self._loop.call_soon(self._hydrate_then_connect)
        return attempt

    def close(self, timeout: float = 2.0) -> None:
        """Stops syncing and releases the connection. Idempotent.

        Terminal: ``connect()`` raises ``CheckgateClosedError`` afterwards.
        Evaluations keep answering from the last flags held (so requests still
        in flight during shutdown are not disrupted). Waits up to ``timeout``
        seconds for the background threads to finish.
        """
        if self._epoch != _ForkEpoch.value:
            # A forked child must not touch locks inherited from the parent.
            self._closed = True
            return
        with self._lock:
            if self._closed:
                return
            self._closed = True
            self._user_closed = True
            self._state = "closed"
            if self._reconnect_timer is not None:
                self._reconnect_timer.cancel()
                self._reconnect_timer = None
            self._stop_poll_fallback()
            self._close_stream()
            # Reset readiness, as the Node client does on disconnect().
            self._ready = False
            self._staged = None
            self._staged_revisions = None
            self._reconnect_attempts = 0
            self._settle(CheckgateClosedError("[Checkgate] close() was called before connect() completed."))
            self._pending_changes.clear()
            self._listeners = ()
            workers = list(self._workers)
        self._loop.stop()
        deadline = time.monotonic() + timeout
        self._loop.join(timeout)
        for worker in workers:
            if worker is not threading.current_thread():
                worker.join(max(0.0, deadline - time.monotonic()))

    def __enter__(self) -> "CheckgateClient":
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    async def __aenter__(self) -> "CheckgateClient":
        return self

    async def __aexit__(self, *exc: object) -> None:
        await asyncio.to_thread(self.close)

    def _check_fork(self) -> None:
        if self._epoch != _ForkEpoch.value:
            raise CheckgateForkError(
                "[Checkgate] This client was created before os.fork() and cannot be used in the "
                "child process. Create the client in each worker (e.g. gunicorn post_fork), "
                "not before the fork.")

    # ------------------------------------------------------------------
    # Status
    # ------------------------------------------------------------------

    def is_ready(self) -> bool:
        """Whether a complete flag set from the server has been applied."""
        return self._ready

    def is_polling(self) -> bool:
        """Whether the client is polling (stream unavailable)."""
        return self._polling

    def get_status(self) -> Status:
        """Connection state, last sync time, and the auth error behind
        ``auth_failed`` (else None). Flags keep being served in every state."""
        self._check_fork()
        with self._lock:
            return {"state": self._state, "last_sync_at_ms": self._last_sync_at, "error": self._status_error}

    def get_health(self) -> Health:
        """Sync diagnostics (schema 1, CONTRACTS.md). Holds no SDK key, user
        id or attribute."""
        self._check_fork()
        with self._lock:
            now = _now_ms()
            applied = [r.revision for r in self._revisions.values()]
            return {
                "schema_version": 1,
                "state": self._state,
                "ready": self._ready,
                "source": self._source,
                "flag_count": self._engine.flag_count(),
                "last_sync_at_ms": self._last_sync_at,
                "config_age_ms": None if self._last_sync_at is None else max(0, now - self._last_sync_at),
                "last_received_revision": _int_or_none(self._last_received_revision),
                "last_applied_revision": _int_or_none(max(applied)) if applied else None,
                "stale_events_ignored": self._stale_events_ignored,
                "reconnect_attempts": self._reconnect_attempts,
                "reconnect_count": self._reconnect_count,
                "polling": self._polling,
                "last_poll_at_ms": self._last_poll_at,
                "last_error": dict(self._health_error) if self._health_error else None,  # type: ignore[typeddict-item]
                "offline_cache": {
                    "configured": self.storage is not None,
                    "hydrated": self._hydrated_from_cache,
                    "last_persist_at_ms": self._last_persist_at,
                },
            }

    def on_change(self, callback: Callable[[str], None]) -> Callable[[], None]:
        """Calls ``callback(flag_key)`` whenever a flag is created, updated or
        deleted after the initial bootstrap. A reconnect re-sync calls it only
        for flags that changed while disconnected. Runs on the sync thread;
        keep it short, and do not call ``connect()`` from it. Exceptions are
        logged. Returns a function that unsubscribes."""
        self._check_fork()
        with self._lock:
            if not self._closed:
                self._listeners = self._listeners + (callback,)

        def unsubscribe() -> None:
            with self._lock:
                self._listeners = tuple(cb for cb in self._listeners if cb is not callback)

        return unsubscribe

    # ------------------------------------------------------------------
    # Evaluation (any thread, no I/O, never waits for the sync thread)
    # ------------------------------------------------------------------

    def _evaluable(self, what: str) -> bool:
        if self._epoch != _ForkEpoch.value:
            self._check_fork()
        if self._ready or self._hydrated:
            return True
        if not self._warned_not_ready:
            self._warned_not_ready = True
            log.warning("[Checkgate] %s called before connect() completed; returning the default.", what)
        return False

    def is_enabled(self, flag_key: str, user_id: str, attributes: Optional[Mapping[str, Any]] = None) -> bool:
        """Whether the flag is on for this user. False for an unknown flag,
        and before the first flag set arrived.

        ``attributes`` values that are not strings are ignored (an attribute
        rule only ever compares strings).
        """
        # The hot path: two attribute checks, then the engine.
        if (self._epoch != _ForkEpoch.value or not (self._ready or self._hydrated)) \
                and not self._evaluable("is_enabled()"):
            return False
        if attributes is not None and type(attributes) is not dict:
            attributes = dict(attributes)
        return self._engine.is_enabled(flag_key, user_id, attributes)

    def get_variant(self, flag_key: str, user_id: str, attributes: Optional[Mapping[str, Any]] = None) -> Optional[Variant]:
        """``Variant(enabled, value)`` for this user, or None for an unknown
        flag (and before the first flag set arrived)."""
        if not self._evaluable("get_variant()"):
            return None
        result = self._engine.get_variant(flag_key, user_id, _as_dict(attributes))
        return None if result is None else Variant(*result)

    def get_value(self, flag_key: str, user_id: str, attributes: Optional[Mapping[str, Any]] = None,
                  default: Any = None) -> Any:
        """The flag's resolved value for this user, or ``default`` for an
        unknown flag."""
        variant = self.get_variant(flag_key, user_id, attributes)
        return default if variant is None else variant.value

    def get_details(self, flag_key: str, user_id: str, attributes: Optional[Mapping[str, Any]] = None,
                    default: Any = None) -> EvaluationDetails:
        """The evaluation with its reason, matched rule, rollout bucket,
        weighted variant and prerequisite trace (``EvaluationDetails``).
        Costs more than ``get_value`` (it records the path): for debugging and
        detailed APIs such as OpenFeature providers."""
        if not self._evaluable("get_details()"):
            return EvaluationDetails(flag_key, default, False, "ERROR", error_code="PROVIDER_NOT_READY",
                                     error_message="no flag set has been loaded yet")
        explanation = self._engine.explain(flag_key, user_id, _as_dict(attributes))
        if explanation is None:
            return EvaluationDetails(flag_key, default, False, "ERROR", error_code="FLAG_NOT_FOUND",
                                     error_message=f"flag {flag_key!r} not found")
        reason = explanation["reason"]
        return EvaluationDetails(
            flag_key=flag_key,
            value=explanation["value"],
            enabled=explanation["enabled"],
            reason=OPENFEATURE_REASONS.get(reason, reason),
            checkgate_reason=reason,
            variant_index=explanation.get("variant_index"),
            rule_index=explanation.get("rule_index"),
            rollout_bucket=explanation.get("rollout_bucket"),
            prerequisites=tuple(explanation.get("prerequisites", ())),
        )

    # ------------------------------------------------------------------
    # Sync thread: task plumbing
    # ------------------------------------------------------------------

    def _task(self, fn: Callable[[], None]) -> None:
        """Runs one state change under the lock, then (outside it) the
        notifications and the snapshot write it queued."""
        with self._lock:
            fn()
            changes = self._pending_changes
            self._pending_changes = []
            listeners = self._listeners
            payload = None
            if self._persist_pending:
                self._persist_pending = False
                if self.storage is not None:
                    payload = json.dumps(self._flag_cache, separators=(",", ":"))
        if payload is not None:
            self._write_cache(payload)
        for key in changes:
            for cb in listeners:
                try:
                    cb(key)
                except Exception:
                    log.exception("[Checkgate] change listener raised")

    def _soon(self, fn: Callable[[], None]) -> None:
        self._loop.call_soon(lambda: self._task(fn))

    def _later(self, ms: float, fn: Callable[[], None]) -> Timer:
        holder: list[Timer] = []

        def run() -> None:
            if not holder[0].cancelled:
                fn()

        timer = self._loop.call_later(ms / 1000.0, lambda: self._task(run))
        holder.append(timer)
        return timer

    def _spawn(self, target: Callable[[], None], name: str) -> None:
        def run() -> None:
            try:
                target()
            finally:
                with self._lock:
                    self._workers.discard(thread)

        thread = threading.Thread(target=run, name=name, daemon=True)
        self._workers.add(thread)
        thread.start()

    def _settle(self, error: Optional[BaseException]) -> None:
        attempt = self._attempt
        self._attempt = None
        if attempt is None:
            return
        attempt.error = error
        attempt.event.set()
        for loop, fut in attempt.waiters:
            try:
                loop.call_soon_threadsafe(_wake, fut, error)
            except RuntimeError:
                pass  # that event loop is closed

    def _record_error(self, kind: ErrorKind, message: str) -> None:
        self._health_error = {"message": message, "kind": kind, "at_ms": _now_ms()}

    def _received(self, revision: Optional[float]) -> None:
        if revision is not None and (self._last_received_revision is None or revision > self._last_received_revision):
            self._last_received_revision = revision

    def _mark_ready(self) -> None:
        self._last_sync_at = _now_ms()
        self._ready = True
        self._source = "server"
        # Also settles a connect() issued after an auth failure, when the
        # client was already ready once.
        self._settle(None)

    # ------------------------------------------------------------------
    # Offline persistence
    # ------------------------------------------------------------------

    def _hydrate_then_connect(self) -> None:
        """First task of a connect(): load the persisted snapshot (so flags
        evaluate offline at once), then open the stream. Unlike the Node
        client, hydration only happens while nothing came from the server
        yet: a reconnect after an auth failure keeps the live flags."""
        raw = None
        with self._lock:
            wants = self.storage is not None and self._source == "none" and not self._user_closed
        if wants:
            try:
                raw = self.storage.get_item(self._cache_key)  # type: ignore[union-attr]
            except Exception as err:
                log.warning("[Checkgate] Failed to read the flag cache: %s", err)

        def apply() -> None:
            if self._user_closed:
                return
            try:
                if raw and self._source == "none":
                    try:
                        snapshot = loads(raw)
                        if not isinstance(snapshot, dict) or not all(isinstance(f, dict) for f in snapshot.values()):
                            raise ValueError("not a flag set")
                    # Anything a corrupt or hostile cache can raise (RecursionError,
                    # TypeError from a custom storage, ...): never stop the connect.
                    except Exception as err:
                        log.warning("[Checkgate] Ignoring an unreadable flag cache: %s", err)
                    else:
                        self._flag_cache.update(snapshot)
                        self._apply_engine([(k, f) for k, f in snapshot.items()], [])
                        # Only mark hydrated once the live stream hasn't already taken over.
                        if not self._ready:
                            self._hydrated = True
                            self._hydrated_from_cache = True
                            self._source = "offline_cache"
            finally:
                # The stream opens whatever happened to the cache (as in Node).
                self._connect_deltas()

        self._task(apply)

    def _persist_cache(self) -> None:
        self._persist_pending = True

    def _write_cache(self, payload: str) -> None:
        try:
            self.storage.set_item(self._cache_key, payload)  # type: ignore[union-attr]
        except Exception as err:
            log.warning("[Checkgate] Failed to persist the flag cache: %s", err)
            return
        with self._lock:
            self._last_persist_at = _now_ms()

    # ------------------------------------------------------------------
    # Engine
    # ------------------------------------------------------------------

    def _apply_engine(self, upserts: list[tuple[str, Any]], deletes: list[str]) -> None:
        """One atomic engine change. A flag the engine cannot parse is not
        served (and no older version of it either: the server replaced
        that); the Node client would keep serving the old version."""
        if not upserts and not deletes:
            return
        rejected = self._engine.apply(
            [(key, json.dumps(flag, separators=(",", ":"))) for key, flag in upserts], deletes)
        for key in rejected:
            log.warning("[Checkgate] Flag %r is not valid for this SDK version and is not served.", key)

    # ------------------------------------------------------------------
    # Stream
    # ------------------------------------------------------------------

    def _connect_deltas(self) -> None:
        self._close_stream()
        # The replay this stream sends is read after this point.
        self._stream_started_at = self._learn_seq
        holder: list[StreamRequest] = []

        def report(kind: str, *args: Any) -> None:
            self._soon(lambda: self._on_stream(holder[0], kind, *args))

        stream = StreamRequest(self._endpoint, self._headers, self.connect_timeout_ms / 1000.0, report)
        holder.append(stream)
        self._stream = stream
        # Until "connected" arrives, only the connect deadline applies.
        self._idle_timeout_ms = 0.0
        self._arm_watchdog(self.connect_timeout_ms)
        self._spawn(stream.run, "checkgate-stream")

    def _on_stream(self, stream: StreamRequest, kind: str, *args: Any) -> None:
        # Every handler ignores a stream that has been replaced or closed.
        if self._stream is not stream:
            return
        if kind == "event":
            name, data = args
            if name not in _KNOWN_EVENTS:
                return
            self._touch()
            getattr(self, f"_on_{name}")(data)
        elif kind == "status":
            (status,) = args
            if status in AUTH_FAILURE_STATUSES:
                self._fail_auth(status)
                return
            self._record_error("http", f"stream answered HTTP {status}")
            self._schedule_reconnect()
        else:
            err_kind, message = args
            self._record_error(err_kind, f"stream: {message}")
            self._schedule_reconnect()

    def _on_connected(self, data: str) -> None:
        # The server sends "connected" before the full-state dump on every
        # (re)connect. The replay is staged and only replaces the live flag
        # set on "ready", so evaluations keep using the last complete set
        # while it streams in, and a replay cut short never goes live.
        self._staged = {}
        self._staged_revisions = {}
        # A successful connection resets the backoff schedule and ends any
        # active poll fallback: stream deltas take over again.
        self._reconnect_attempts = 0
        self._stop_poll_fallback()
        log.debug("[Checkgate] Stream connected; rebuilding from server state.")
        try:
            payload = loads(data)
        except (ValueError, RecursionError):
            payload = None
        payload = payload if isinstance(payload, dict) else {}
        env_id = payload.get("environment_id")
        self._env_id = env_id if isinstance(env_id, str) and env_id else None
        # From now on the stream must keep talking: the server sends a
        # heartbeat whenever it has been quiet for `heartbeat_ms`.
        self._idle_timeout_ms = idle_timeout_for(payload.get("heartbeat_ms"), self.stream_idle_timeout_ms)
        if self._idle_timeout_ms > 0:
            self._arm_watchdog(self._idle_timeout_ms)
        else:
            self._disarm_watchdog()

    def _on_heartbeat(self, data: str) -> None:
        pass  # liveness only; _touch() has recorded it

    def _on_ready(self, data: str) -> None:
        staged = self._staged
        if staged is None:
            return
        revisions = self._staged_revisions
        self._staged = None
        self._staged_revisions = None
        # Swap in the complete replay, except where a poll that completed
        # while it was on its way is newer (R20).
        self._apply_snapshot(staged, revisions, self._stream_started_at)
        self._hydrated = True
        self._persist_cache()
        self._state = "streaming"
        log.debug("[Checkgate] Stream ready: %d flags.", len(self._flag_cache))
        self._mark_ready()

    def _on_update(self, data: str) -> None:
        try:
            event = loads(data)
        except (ValueError, RecursionError) as err:  # deep nesting raises RecursionError
            log.error("[Checkgate] Failed to parse a delta update: %s", err)
            self._record_error("parse", "stream: an update event was not valid JSON")
            return
        self._apply_delta(event)

    def _touch(self) -> None:
        """Records that the current stream is alive, and pushes its deadline out."""
        if self._idle_timeout_ms > 0:
            self._watchdog_deadline = time.monotonic() + self._idle_timeout_ms / 1000.0
        # A live, bootstrapped stream with no news means the flags are current.
        if self._state == "streaming" and self._staged is None:
            self._last_sync_at = _now_ms()

    def _arm_watchdog(self, ms: float) -> None:
        """Declares the current stream dead `ms` from now unless _touch() moves that."""
        deadline = time.monotonic() + ms / 1000.0
        self._watchdog_deadline = deadline
        if self._watchdog is not None and self._watchdog_fire_at <= deadline:
            return
        if self._watchdog is not None:
            self._watchdog.cancel()
        self._schedule_watchdog(ms / 1000.0)

    def _schedule_watchdog(self, seconds: float) -> None:
        self._watchdog_fire_at = time.monotonic() + seconds
        self._watchdog = self._later(seconds * 1000.0, self._on_watchdog)

    def _on_watchdog(self) -> None:
        self._watchdog = None
        if self._stream is None or self._watchdog_deadline is None:
            return
        left = self._watchdog_deadline - time.monotonic()
        if left > 0:
            self._schedule_watchdog(left)
            return
        log.warning("[Checkgate] Stream silent past its deadline; treating it as dead and reconnecting.")
        self._record_error("timeout", "stream: no event before the deadline")
        self._schedule_reconnect()

    def _disarm_watchdog(self) -> None:
        self._watchdog_deadline = None
        if self._watchdog is not None:
            self._watchdog.cancel()
            self._watchdog = None

    def _close_stream(self) -> None:
        self._disarm_watchdog()
        stream = self._stream
        self._stream = None
        if stream is not None:
            stream.cancel()

    def _fail_auth(self, status: int) -> None:
        """The server rejected the SDK key: stop every retry (stream and
        poll), keep serving the flags already loaded, and settle connect()."""
        err = CheckgateAuthError(status)
        log.error("%s", err)
        self._close_stream()
        if self._reconnect_timer is not None:
            self._reconnect_timer.cancel()
            self._reconnect_timer = None
        self._stop_poll_fallback()
        self._staged = None
        self._staged_revisions = None
        self._reconnect_attempts = 0
        self._state = "auth_failed"
        self._status_error = err
        self._record_error("auth", f"the server rejected the SDK key (HTTP {status})")
        self._settle(err)

    def _apply_delta(self, event: Any) -> None:
        """Applies one stream change event. While a (re)connect replay is in
        progress the event is staged for "ready"; afterwards it is applied
        unless it is older than the revision held for that flag."""
        if not isinstance(event, dict):
            return
        is_upsert = event.get("type") == "UPSERT"
        if not is_upsert and event.get("type") != "DELETE":
            return
        flag = event.get("flag") if is_upsert else None
        key = (flag.get("key") if isinstance(flag, dict) else None) if is_upsert else event.get("key")
        if not isinstance(key, str):
            return
        revision = event_revision(event.get("revision"))
        self._received(revision)

        if self._staged is not None and self._staged_revisions is not None:
            if is_stale_event(self._staged_revisions.get(key), revision, is_upsert):
                self._stale_events_ignored += 1
                return
            if is_upsert:
                self._staged[key] = flag
            else:
                self._staged.pop(key, None)
            if revision is not None:
                self._staged_revisions[key] = Rev(revision, not is_upsert)
            return

        if is_stale_event(self._revisions.get(key), revision, is_upsert):
            self._stale_events_ignored += 1
            return
        if revision is not None:
            self._learn_seq += 1
            self._revisions[key] = Rev(revision, not is_upsert, self._learn_seq)
        if is_upsert:
            self._apply_engine([(key, flag)], [])
            self._flag_cache[key] = flag
        else:
            # A delete of a flag the client does not hold changes nothing.
            if key not in self._flag_cache:
                return
            del self._flag_cache[key]
            self._apply_engine([], [key])
        self._pending_changes.append(key)
        self._persist_cache()  # keep the offline snapshot fresh

    def _schedule_reconnect(self) -> None:
        """Closes the current stream and schedules a reconnect with
        exponential backoff and up to 30 % jitter."""
        if self._user_closed or self._reconnect_timer is not None or self._state == "auth_failed":
            return
        self._close_stream()
        base = self.reconnect_delay_ms * (2.0 ** min(self._reconnect_attempts, 60))
        capped = min(self.max_reconnect_delay_ms, base)
        delay = capped + random.random() * capped * 0.3
        self._reconnect_attempts += 1
        self._reconnect_count += 1
        # A replay cut short never replaces the live snapshot.
        self._staged = None
        self._staged_revisions = None
        # The stream is failing repeatedly (e.g. a proxy blocking long-lived
        # connections): also poll, so flags stay current. Stream retries go
        # on; a successful reconnect stops polling again.
        if self._reconnect_attempts >= self.poll_fallback_threshold:
            self._start_poll_fallback()
        self._state = "polling" if self._polling else ("reconnecting" if self._ready else "connecting")
        log.info("[Checkgate] Stream disconnected; reconnecting in %d ms.", round(delay))

        def fire() -> None:
            self._reconnect_timer = None
            if not self._user_closed:
                self._connect_deltas()

        self._reconnect_timer = self._later(delay, fire)

    # ------------------------------------------------------------------
    # Poll fallback
    # ------------------------------------------------------------------

    def _start_poll_fallback(self) -> None:
        if self._polling:
            return
        self._polling = True
        log.warning("[Checkgate] Stream reconnects failing; polling %s/flags/snapshot every %d ms.",
                    self.server_url, round(self.poll_interval_ms))
        self._poll_snapshot()  # now, rather than after the first interval
        self._schedule_poll()

    def _schedule_poll(self) -> None:
        def tick() -> None:
            if not self._polling:
                return
            self._poll_snapshot()
            self._schedule_poll()

        self._poll_timer = self._later(self.poll_interval_ms, tick)

    def _stop_poll_fallback(self) -> None:
        self._sync_generation += 1
        if self._poll_request is not None:
            self._poll_request.cancel()
        self._poll_request = None
        self._polling = False
        if self._poll_timer is not None:
            self._poll_timer.cancel()
            self._poll_timer = None

    def _poll_snapshot(self) -> None:
        if self._user_closed or self._poll_request is not None:
            return
        generation = self._sync_generation
        # The server reads the snapshot after this point (see merge_full_set).
        started_at = self._learn_seq
        holder: list[PollRequest] = []

        def report(kind: str, *args: Any) -> None:
            self._soon(lambda: self._on_poll(holder[0], generation, started_at, kind, *args))

        request = PollRequest(self._endpoint, self._headers, self.request_timeout_ms / 1000.0, report, loads)
        holder.append(request)
        self._poll_request = request
        self._spawn(request.run, "checkgate-poll")

    def _on_poll(self, request: Request, generation: int, started_at: int, kind: str, *args: Any) -> None:
        if self._poll_request is request:
            self._poll_request = None
        if generation != self._sync_generation or self._user_closed:
            return
        if kind == "status":
            (status,) = args
            if status in AUTH_FAILURE_STATUSES:
                self._fail_auth(status)
                return
            log.warning("[Checkgate] Poll request failed: HTTP %s", status)
            self._record_error("http", f"poll answered HTTP {status}")
            return
        if kind == "error":
            err_kind, message = args
            log.warning("[Checkgate] Poll request failed: %s", message)
            self._record_error(err_kind, f"poll: {message}")
            return
        body, env_id = args
        try:
            flags, revisions = parse_snapshot(body)
        except (ValueError, TypeError, RecursionError) as err:
            log.warning("[Checkgate] Poll returned an unusable snapshot: %s", err)
            self._record_error("parse", f"poll: {err}")
            return
        if env_id:
            self._env_id = env_id
        self._apply_snapshot(flags, revisions, started_at)
        self._hydrated = True
        self._last_poll_at = _now_ms()
        self._mark_ready()

    # ------------------------------------------------------------------
    # Full flag sets
    # ------------------------------------------------------------------

    def _apply_snapshot(self, incoming: dict[str, Any], revisions: Optional[dict[str, Rev]],
                        started_at: int) -> bool:
        """Applies a complete flag set (a replay or a poll), merged with what
        the client holds by merge_full_set, as one atomic engine change; then
        notifies listeners of the flags that actually changed (only after the
        whole set is in place: a listener evaluating a flag must not see some
        prerequisites updated and others not). Returns whether anything
        changed.

        Flag versions are compared independent of key order (the Node client
        compares serialized JSON, which also reports a key-order difference
        between stream and snapshot as a change)."""
        if revisions is not None:
            for rev in revisions.values():
                self._received(rev.revision)
        self._learn_seq += 1
        merged_flags, merged_revisions, rejected = merge_full_set(
            self._flag_cache, self._revisions, incoming, revisions, started_at, self._learn_seq)
        self._stale_events_ignored += rejected
        upserts: list[tuple[str, Any]] = []
        changed: list[str] = []
        for key, flag in merged_flags.items():
            prev = self._flag_cache.get(key)
            if prev is not None and (prev is flag or canonical(prev) == canonical(flag)):
                self._flag_cache[key] = flag  # same version; keep the server's key order
                continue
            upserts.append((key, flag))
            changed.append(key)
        deletes = [key for key in self._flag_cache if key not in merged_flags]
        changed.extend(deletes)
        self._apply_engine(upserts, deletes)
        for key, flag in upserts:
            self._flag_cache[key] = flag
        for key in deletes:
            del self._flag_cache[key]
        self._revisions = merged_revisions
        if not changed:
            return False
        self._persist_cache()
        if self._ready:
            self._pending_changes.extend(changed)
        return True

    # ------------------------------------------------------------------
    # Introspection for tests and the conformance adapter (not public API)
    # ------------------------------------------------------------------

    def _debug_flags(self) -> dict[str, Any]:
        with self._lock:
            return dict(self._flag_cache)

    def _debug_sample(self, users: list[str], attributes: Optional[Mapping[str, Any]] = None
                      ) -> tuple[dict[str, Any], dict[str, bool]]:
        """Flags and evaluations at one instant (no change applied between)."""
        with self._lock:
            flags = dict(self._flag_cache)
            attrs = _as_dict(attributes)
            evals = {f"{key}/{user}": self._engine.is_enabled(key, user, attrs) for key in flags for user in users}
        return flags, evals


def _as_dict(attributes: Optional[Mapping[str, Any]]) -> Optional[dict[str, Any]]:
    if attributes is None:
        return None
    if type(attributes) is dict:
        return cast("dict[str, Any]", attributes)
    return dict(attributes)


def _int_or_none(value: Optional[float]) -> Optional[int]:
    return None if value is None else int(value)
