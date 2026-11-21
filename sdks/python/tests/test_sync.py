"""The sync state machine against a local fake server (tests/fake_server.py):
bootstrap, deltas, ordering, reconnects, polling, R20, auth, watchdog,
offline cache, lifecycle and health."""

from __future__ import annotations

import asyncio
import json
import math
import threading
import time
from typing import Any, Callable

import pytest
from fake_server import ENV_ID, FakeServer, flag, frame, wait_until

from checkgate import (
    CheckgateAuthError,
    CheckgateClient,
    CheckgateClosedError,
    CheckgateTimeoutError,
    FileStorage,
    MemoryStorage,
)
from checkgate import _client as client_module

Make = Callable[..., CheckgateClient]


def flags_of(c: CheckgateClient) -> dict[str, Any]:
    return c._debug_flags()


def test_bootstrap_and_health(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("on"))
    server.upsert(flag("off", enabled=False))
    server.delete("off")
    server.upsert(flag("ruled", rollout_percentage=0,
                       rules=[{"attribute": "plan", "operator": "equals", "values": ["pro"]}]))
    c = make_client(server.url)
    assert c.get_status()["state"] == "idle"
    assert c.is_enabled("on", "u") is False  # nothing loaded yet: default
    c.connect(timeout=5)
    assert c.is_ready()
    assert c.is_enabled("on", "u") is True
    assert c.is_enabled("off", "u") is False
    assert c.is_enabled("ruled", "u", {"plan": "pro"}) is True
    assert c.is_enabled("ruled", "u", {"plan": "free"}) is False
    assert c.get_status()["state"] == "streaming"
    h = c.get_health()
    assert h["schema_version"] == 1 and h["ready"] is True and h["source"] == "server"
    assert h["flag_count"] == 2 and h["state"] == "streaming"
    assert h["last_applied_revision"] == 4 and h["last_received_revision"] == 4
    assert h["last_sync_at_ms"] is not None and h["config_age_ms"] is not None and h["config_age_ms"] >= 0
    assert h["stale_events_ignored"] == 0 and h["reconnect_attempts"] == 0 and h["polling"] is False
    assert h["offline_cache"] == {"configured": False, "hydrated": False, "last_persist_at_ms": None}
    assert h["last_error"] is None
    assert set(h) == {
        "schema_version", "state", "ready", "source", "flag_count", "last_sync_at_ms", "config_age_ms",
        "last_received_revision", "last_applied_revision", "stale_events_ignored", "reconnect_attempts",
        "reconnect_count", "polling", "last_poll_at_ms", "last_error", "offline_cache"}
    text = json.dumps(h)
    assert "sk_test" not in text and "pro" not in text
    # A heartbeat counts as a sync.
    before = h["last_sync_at_ms"]
    assert wait_until(lambda: c.get_health()["last_sync_at_ms"] > before, 2)


def test_deltas_and_change_listeners(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("a"))
    c = make_client(server.url)
    seen: list[str] = []
    unsubscribe = c.on_change(seen.append)
    c.on_change(lambda key: 1 / 0)  # a failing listener breaks nothing
    c.connect(timeout=5)
    assert seen == []  # the bootstrap does not notify
    server.upsert(flag("a", enabled=False))
    assert wait_until(lambda: c.is_enabled("a", "u") is False)
    server.upsert(flag("b", flag_type="string", default_value="x"))
    assert wait_until(lambda: c.get_value("b", "u") == "x")
    server.delete("a")
    assert wait_until(lambda: "a" not in flags_of(c))
    assert wait_until(lambda: seen == ["a", "b", "a"])
    unsubscribe()
    server.upsert(flag("c"))
    assert wait_until(lambda: "c" in flags_of(c))
    time.sleep(0.05)
    assert seen == ["a", "b", "a"]


def test_stale_duplicate_and_out_of_order_events_are_ignored(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("a", description="v1"))  # rev 1
    server.upsert(flag("a", description="v2"))  # rev 2
    c = make_client(server.url)
    changes: list[str] = []
    c.on_change(changes.append)
    c.connect(timeout=5)
    old = {"type": "UPSERT", "env_id": ENV_ID, "flag": flag("a", description="v1"), "revision": 1}
    dup = {"type": "UPSERT", "env_id": ENV_ID, "flag": flag("a", description="v2"), "revision": 2}
    old_delete = {"type": "DELETE", "env_id": ENV_ID, "key": "a", "revision": 1}
    for event in (old, dup, old_delete):
        server.send(frame("update", event))
    server.upsert(flag("marker"))  # rev 3, arrives after the three
    assert wait_until(lambda: "marker" in flags_of(c))
    assert flags_of(c)["a"]["description"] == "v2"
    assert c.get_health()["stale_events_ignored"] == 3
    assert changes == ["marker"]
    # A delete of exactly the held version applies, and a later older upsert
    # cannot resurrect it.
    server.send(frame("update", {"type": "DELETE", "env_id": ENV_ID, "key": "a", "revision": 2}))
    server.send(frame("update", old))
    server.upsert(flag("marker2"))
    assert wait_until(lambda: "marker2" in flags_of(c))
    assert "a" not in flags_of(c)
    # Garbage is ignored (and recorded), not fatal.
    server.send("event: update\ndata: {not json\n\n")
    server.upsert(flag("marker3"))
    assert wait_until(lambda: "marker3" in flags_of(c))
    assert c.get_health()["last_error"]["kind"] == "parse"


def test_malformed_flag_from_the_stream_is_not_served(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("a"))
    c = make_client(server.url)
    c.connect(timeout=5)
    assert c.is_enabled("a", "u")
    server.upsert({"key": "a", "is_enabled": True, "rollout_percentage": "50", "description": None})
    # Fail closed: the old version is not kept either.
    assert wait_until(lambda: c.get_variant("a", "u") is None)
    assert c.get_details("a", "u").error_code == "FLAG_NOT_FOUND"


def test_reconnect_resyncs_and_notifies_only_changes(server: FakeServer, make_client: Make) -> None:
    for k in ("a", "b", "c"):
        server.upsert(flag(k))
    c = make_client(server.url)
    changes: list[str] = []
    c.on_change(changes.append)
    c.connect(timeout=5)
    server.stream_mode = "refuse"
    server.sever_all()
    assert wait_until(lambda: c.get_status()["state"] in ("reconnecting", "polling"))
    server.upsert(flag("b", enabled=False), broadcast=False)
    server.delete("c", broadcast=False)
    server.upsert(flag("d"), broadcast=False)
    server.stream_mode = "pass"
    assert wait_until(lambda: c.get_status()["state"] == "streaming" and "d" in flags_of(c), 5)
    assert c.is_enabled("b", "u") is False and "c" not in flags_of(c)
    assert sorted(changes) == ["b", "c", "d"]
    h = c.get_health()
    assert h["reconnect_count"] >= 1 and h["reconnect_attempts"] == 0
    assert h["last_error"] is not None and h["last_error"]["kind"] in ("network", "timeout")
    assert wait_until(lambda: server.open_streams() == 1)


def test_backoff_schedule_is_exponential_capped_with_jitter(monkeypatch: pytest.MonkeyPatch) -> None:
    c = CheckgateClient("http://127.0.0.1:9", "k", reconnect_delay_ms=100, max_reconnect_delay_ms=1000,
                        poll_fallback_threshold=None)
    delays: list[float] = []
    monkeypatch.setattr(c, "_later", lambda ms, fn: delays.append(ms) or client_module.Timer(fn))
    for jitter, expect in ((0.0, [100, 200, 400, 800, 1000, 1000]), (1.0, [130, 260, 520, 1040, 1300, 1300])):
        monkeypatch.setattr(client_module.random, "random", lambda j=jitter: j)
        delays.clear()
        c._reconnect_attempts = 0
        for _ in range(6):
            c._reconnect_timer = None
            c._schedule_reconnect()
        assert [round(d) for d in delays] == expect
    assert c.get_health()["reconnect_count"] == 12
    c.close()


def test_replay_cut_mid_way_keeps_the_previous_set(server: FakeServer, make_client: Make) -> None:
    for k in ("a", "b", "c", "d"):
        server.upsert(flag(k))
    c = make_client(server.url)
    c.connect(timeout=5)
    server.stream_mode = "refuse"
    server.sever_all()
    assert wait_until(lambda: c.get_status()["state"] != "streaming")
    server.upsert(flag("a", enabled=False), broadcast=False)
    server.delete("d", broadcast=False)
    server.cut_next_replay = True
    seen_partial: list[str] = []
    stop = threading.Event()

    def sample() -> None:
        while not stop.is_set():
            fl = flags_of(c)
            # Either the whole old set or the whole new one.
            if not ((fl["a"]["is_enabled"] and "d" in fl) or (not fl["a"]["is_enabled"] and "d" not in fl)):
                seen_partial.append(json.dumps(fl))
            time.sleep(0.002)

    t = threading.Thread(target=sample)
    t.start()
    server.stream_mode = "pass"
    assert wait_until(lambda: c.get_status()["state"] == "streaming" and "d" not in flags_of(c), 5)
    stop.set()
    t.join()
    assert server.counters["streams_opened"] >= 3  # first, the cut one, the recovery
    assert seen_partial == []
    assert c.is_enabled("a", "u") is False


def test_poll_fallback_and_recovery(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("a"))
    c = make_client(server.url)
    c.connect(timeout=5)
    server.stream_mode = "unavailable"
    server.sever_all()
    assert wait_until(lambda: c.get_status()["state"] == "polling", 5)
    assert c.is_polling() and c.get_health()["polling"]
    server.upsert(flag("a", enabled=False), broadcast=False)
    assert wait_until(lambda: c.is_enabled("a", "u") is False, 3)
    h = c.get_health()
    assert h["last_poll_at_ms"] is not None and h["last_error"]["kind"] == "http"
    server.stream_mode = "pass"
    assert wait_until(lambda: c.get_status()["state"] == "streaming", 5)
    assert not c.is_polling()
    polls = server.counters["snapshot_requests"]
    time.sleep(0.3)
    assert server.counters["snapshot_requests"] == polls  # polling stopped


def test_poll_only_bootstrap_resolves_connect(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("a"))
    server.stream_mode = "unavailable"
    c = make_client(server.url)
    c.connect(timeout=5)  # settled by a poll: the stream never connected
    assert c.is_enabled("a", "u") and c.get_status()["state"] == "polling"


def test_legacy_snapshot_server_converges() -> None:
    server = FakeServer(heartbeat_ms=100, legacy_snapshot=True)
    url = server.start()
    c = CheckgateClient(url, "sk_test", reconnect_delay_ms=20, max_reconnect_delay_ms=200,
                        poll_interval_ms=100, connect_timeout_ms=500)
    try:
        server.upsert(flag("a"))
        server.upsert(flag("b"))
        c.connect(timeout=5)
        server.stream_mode = "unavailable"
        server.sever_all()
        assert wait_until(lambda: c.is_polling(), 5)
        server.upsert(flag("a", enabled=False), broadcast=False)
        server.delete("b", broadcast=False)
        assert wait_until(lambda: not c.is_enabled("a", "u") and "b" not in flags_of(c), 3)
        server.stream_mode = "pass"
        assert wait_until(lambda: c.get_status()["state"] == "streaming", 5)
    finally:
        c.close()
        server.stop()


def test_delayed_replay_does_not_roll_back_newer_polled_flags_r20(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("changed", description="before"))
    server.upsert(flag("deleted"))
    c = make_client(server.url)
    c.connect(timeout=5)
    server.stream_mode = "unavailable"
    server.sever_all()
    assert wait_until(lambda: c.is_polling(), 5)
    # The next stream reads its replay when it opens, then holds its bytes.
    server.hold_next_stream = 0.35  # under the 500 ms connect deadline
    server.stream_mode = "pass"
    assert server.held_stream_opened.wait(5)
    server.upsert(flag("changed", description="after"))
    server.delete("deleted")
    assert wait_until(lambda: flags_of(c)["changed"]["description"] == "after" and "deleted" not in flags_of(c), 3)
    rollbacks: list[str] = []
    end = time.monotonic() + 3
    streaming_at = None
    while time.monotonic() < end:
        fl = flags_of(c)
        if fl.get("changed", {}).get("description") != "after":
            rollbacks.append("changed")
        if "deleted" in fl:
            rollbacks.append("deleted")
        if streaming_at is None and c.get_status()["state"] == "streaming":
            streaming_at = time.monotonic()
        if streaming_at is not None and time.monotonic() - streaming_at > 0.5:
            break
        time.sleep(0.002)
    assert streaming_at is not None, "the held stream never took over"
    assert rollbacks == []
    assert c.get_health()["stale_events_ignored"] >= 1


def test_rejected_key_is_terminal_and_recovers_on_connect(server: FakeServer, make_client: Make) -> None:
    bad = make_client(server.url, sdk_key="wrong")
    with pytest.raises(CheckgateAuthError) as info:
        bad.connect(timeout=5)
    assert info.value.status == 401
    status = bad.get_status()
    assert status["state"] == "auth_failed" and isinstance(status["error"], CheckgateAuthError)
    assert bad.get_health()["last_error"]["kind"] == "auth"
    n = server.counters["stream_requests"] + server.counters["snapshot_requests"]
    time.sleep(0.5)
    assert server.counters["stream_requests"] + server.counters["snapshot_requests"] == n


def test_revoked_key_keeps_flags_and_reconnects_after_restore(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("a"))
    c = make_client(server.url)
    c.connect(timeout=5)
    server.accepted_key = None
    server.sever_all()
    assert wait_until(lambda: c.get_status()["state"] == "auth_failed", 5)
    assert c.is_enabled("a", "u") is True  # last flags kept
    n = server.counters["stream_requests"] + server.counters["snapshot_requests"]
    time.sleep(0.4)
    assert server.counters["stream_requests"] + server.counters["snapshot_requests"] == n
    with pytest.raises(CheckgateAuthError):
        c.connect(timeout=5)  # still rejected
    server.accepted_key = server.sdk_key
    server.upsert(flag("a", enabled=False), broadcast=False)
    c.connect(timeout=5)
    assert c.get_status() == {"state": "streaming", "last_sync_at_ms": c.get_status()["last_sync_at_ms"], "error": None}
    assert c.is_enabled("a", "u") is False


def test_idle_watchdog_replaces_a_silent_stream(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("a"))
    c = make_client(server.url)
    c.connect(timeout=5)
    opened = server.counters["streams_opened"]
    stalled_at = time.monotonic()
    server.stall_all()
    server.upsert(flag("a", enabled=False), broadcast=False)
    assert wait_until(lambda: server.counters["streams_opened"] > opened, 3)
    detect = server.stream_opened_at[-1] - stalled_at
    assert detect < 0.3 + 0.4, detect  # 3 heartbeats + scheduling margin
    assert wait_until(lambda: c.is_enabled("a", "u") is False, 3)
    assert c.get_health()["last_error"]["kind"] == "timeout"


def test_quiet_stream_without_heartbeat_announcement_is_kept(make_client: Make) -> None:
    server = FakeServer(heartbeat_ms=None)  # a pre-0.1.31 server
    url = server.start()
    try:
        server.upsert(flag("a"))
        c = make_client(url)
        c.connect(timeout=5)
        time.sleep(1.5)
        assert server.counters["streams_opened"] == 1
        assert c.get_status()["state"] == "streaming"
    finally:
        server.stop()


def test_quiet_healthy_stream_is_kept(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("a"))
    c = make_client(server.url)
    c.connect(timeout=5)
    time.sleep(1.5)
    assert server.counters["streams_opened"] == 1


def test_connect_deadline_abandons_a_stream_that_never_says_connected(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("a"))
    server.stream_mode = "silent"
    c = make_client(server.url, connect_timeout_ms=200, poll_fallback_threshold=None)
    with pytest.raises(CheckgateTimeoutError):
        c.connect(timeout=0.8)
    assert server.counters["streams_opened"] >= 2  # abandoned and retried
    assert c.get_health()["last_error"]["kind"] == "timeout"
    server.stream_mode = "pass"
    c.connect(timeout=5)  # kept trying in the background
    assert c.is_enabled("a", "u")


def test_offline_cache_hydrates_before_connecting(server: FakeServer, make_client: Make, tmp_path: Any) -> None:
    storage = FileStorage(tmp_path / "cache")
    server.upsert(flag("a"))
    server.upsert(flag("b", flag_type="string", default_value="blue"))
    first = make_client(server.url, storage=storage)
    first.connect(timeout=5)
    assert wait_until(lambda: first.get_health()["offline_cache"]["last_persist_at_ms"] is not None)
    server.upsert(flag("a", enabled=False))
    assert wait_until(lambda: first.is_enabled("a", "u") is False)
    time.sleep(0.1)
    first.close()
    files = list((tmp_path / "cache").iterdir())
    assert len(files) == 1 and "sk_test" not in files[0].read_text()

    server.stream_mode = server.snapshot_mode = "refuse"
    second = make_client(server.url, storage=storage)
    with pytest.raises(CheckgateTimeoutError):
        second.connect(timeout=0.5)
    assert second.is_enabled("a", "u") is False and second.get_value("b", "u") == "blue"
    h = second.get_health()
    assert h["source"] == "offline_cache" and h["offline_cache"]["hydrated"] and not h["ready"]
    server.stream_mode = server.snapshot_mode = "pass"
    server.upsert(flag("c"), broadcast=False)
    second.connect(timeout=5)
    assert second.get_health()["source"] == "server" and second.is_enabled("c", "u")


def test_unreadable_offline_cache_is_ignored(server: FakeServer, make_client: Make) -> None:
    storage = MemoryStorage()
    c0 = CheckgateClient(server.url, "sk_test")
    storage.set_item(c0._cache_key, "{not json")
    server.upsert(flag("a"))
    c = make_client(server.url, storage=storage)
    c.connect(timeout=5)
    assert c.is_enabled("a", "u") and c.get_health()["offline_cache"]["hydrated"] is False


def test_close_during_connect(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("a"))
    server.stream_mode = "silent"
    c = make_client(server.url, connect_timeout_ms=5000)
    errors: list[BaseException] = []

    def run() -> None:
        try:
            c.connect()
        except BaseException as err:
            errors.append(err)

    t = threading.Thread(target=run)
    t.start()
    assert wait_until(lambda: server.open_streams() == 1)
    started = time.monotonic()
    c.close()
    t.join(2)
    assert not t.is_alive() and isinstance(errors[0], CheckgateClosedError)
    assert time.monotonic() - started < 2
    assert wait_until(lambda: server.open_streams() == 0, 2)
    assert c.get_status()["state"] == "closed"


def test_use_after_close(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("a"))
    c = make_client(server.url)
    c.connect(timeout=5)
    c.close()
    c.close()  # idempotent
    assert wait_until(lambda: server.open_streams() == 0, 2)
    with pytest.raises(CheckgateClosedError):
        c.connect()
    with pytest.raises(CheckgateClosedError):
        asyncio.run(c.connect_async())
    # Evaluations keep answering from the last flags held.
    assert c.is_enabled("a", "u") is True
    assert c.get_details("a", "u").reason == "STATIC"
    assert c.get_health()["state"] == "closed" and c.get_health()["ready"] is False
    calls: list[str] = []
    c.on_change(calls.append)()  # registering is a no-op, unsubscribing too
    server.upsert(flag("a", enabled=False))
    time.sleep(0.2)
    assert c.is_enabled("a", "u") is True and calls == []
    assert not c._loop._thread.is_alive()  # the sync thread has ended


def test_context_manager_closes(server: FakeServer) -> None:
    server.upsert(flag("a"))
    with CheckgateClient(server.url, "sk_test") as c:
        c.connect(timeout=5)
        assert c.is_enabled("a", "u")
    assert c.get_status()["state"] == "closed"
    assert wait_until(lambda: server.open_streams() == 0, 2)


def test_connect_async_does_not_block_the_event_loop(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("a"))
    server.stream_mode = server.snapshot_mode = "refuse"
    c = make_client(server.url)

    async def main() -> int:
        ticks = 0

        async def ticker() -> None:
            nonlocal ticks
            while True:
                ticks += 1
                await asyncio.sleep(0.01)

        task = asyncio.create_task(ticker())
        with pytest.raises(CheckgateTimeoutError):
            await c.connect_async(timeout=0.5)
        assert isinstance(CheckgateTimeoutError(), TimeoutError)
        server.stream_mode = server.snapshot_mode = "pass"
        await c.connect_async(timeout=5)
        task.cancel()
        return ticks

    ticks = asyncio.run(main())
    assert ticks >= 30  # the loop kept running while connect waited
    assert c.is_enabled("a", "u")


def test_connect_async_with_auth_failure(server: FakeServer, make_client: Make) -> None:
    c = make_client(server.url, sdk_key="nope")
    with pytest.raises(CheckgateAuthError):
        asyncio.run(c.connect_async(timeout=5))


def test_details_and_reasons(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("half", rollout_percentage=50))
    server.upsert(flag("off", enabled=False))
    c = make_client(server.url)
    d = c.get_details("half", "u", default=False)
    assert d.reason == "ERROR" and d.error_code == "PROVIDER_NOT_READY" and d.value is False
    c.connect(timeout=5)
    d = c.get_details("half", "user-1")
    assert d.reason == "SPLIT" and d.rollout_bucket is not None
    assert d.checkgate_reason in ("SPLIT", "ROLLOUT_EXCLUDED")
    assert c.get_details("off", "u").reason == "DISABLED"
    missing = c.get_details("nope", "u", default=7)
    assert (missing.reason, missing.error_code, missing.value, missing.enabled) == ("ERROR", "FLAG_NOT_FOUND", 7, False)
    assert c.get_variant("off", "u") == (False, False)


def test_listener_may_close_the_client(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("a"))
    c = make_client(server.url)
    done = threading.Event()

    def listener(key: str) -> None:
        c.close()
        done.set()

    c.on_change(listener)
    c.connect(timeout=5)
    server.upsert(flag("a", enabled=False))
    assert done.wait(3)
    assert c.get_status()["state"] == "closed"
    assert wait_until(lambda: server.open_streams() == 0, 2)


def test_connect_from_a_listener_is_refused(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("a"))
    c = make_client(server.url)
    errors: list[BaseException] = []

    def listener(key: str) -> None:
        try:
            c.connect()
        except RuntimeError as err:
            errors.append(err)

    c.on_change(listener)
    c.connect(timeout=5)
    server.upsert(flag("b"))
    assert wait_until(lambda: len(errors) == 1)


def test_invalid_options() -> None:
    with pytest.raises(ValueError):
        CheckgateClient("ftp://x")
    with pytest.raises(ValueError):
        CheckgateClient("http://x", reconnect_delay_ms=0)
    with pytest.raises(TypeError):
        CheckgateClient("http://x", storage=object())  # type: ignore[arg-type]


def test_hostile_offline_cache_never_stops_the_connect(server: FakeServer, make_client: Make) -> None:
    # Review F-04: only ValueError was caught, so a cache whose parse raised
    # anything else (deep nesting -> RecursionError) skipped opening the
    # stream and connect() never settled.
    c0 = CheckgateClient(server.url, "sk_test")
    server.upsert(flag("a"))
    for raw in ("[" * 100_000 + "]" * 100_000, '{"a": ' + "[" * 50_000 + "]" * 50_000 + "}"):
        storage = MemoryStorage()
        storage.set_item(c0._cache_key, raw)
        c = make_client(server.url, storage=storage)
        c.connect(timeout=5)
        assert c.is_enabled("a", "u")
        assert c.get_health()["offline_cache"]["hydrated"] is False

    class Exploding(MemoryStorage):
        def get_item(self, key: str) -> Any:
            raise TypeError("storage bug")

    c = make_client(server.url, storage=Exploding())
    c.connect(timeout=5)
    assert c.is_enabled("a", "u")


def test_delay_options_are_bounded() -> None:
    # Review F-03: stream_idle_timeout_ms=inf used to pass and then stop the
    # sync thread for good (a timer beyond threading.TIMEOUT_MAX).
    for bad in (math.inf, -1, math.nan, 2**31, "30"):
        with pytest.raises(ValueError):
            CheckgateClient("http://x", stream_idle_timeout_ms=bad)  # type: ignore[arg-type]
    for name in ("reconnect_delay_ms", "max_reconnect_delay_ms", "poll_interval_ms",
                 "connect_timeout_ms", "request_timeout_ms"):
        with pytest.raises(ValueError):
            CheckgateClient("http://x", **{name: math.inf})
        with pytest.raises(ValueError):
            CheckgateClient("http://x", **{name: 2**31})
    CheckgateClient("http://x", stream_idle_timeout_ms=0).close()
    CheckgateClient("http://x", stream_idle_timeout_ms=2**31 - 1, poll_interval_ms=2**31 - 1).close()


def test_a_huge_timer_does_not_end_the_sync_thread() -> None:
    from checkgate._loop import Loop

    loop = Loop("t")
    loop.start()
    ran = threading.Event()
    loop.call_later(1e12, lambda: None)  # far beyond threading.TIMEOUT_MAX seconds
    loop.call_soon(ran.set)
    assert ran.wait(2)
    ran.clear()
    loop.call_soon(ran.set)
    assert ran.wait(2), "the loop must still run tasks after waiting on a huge timer"
    loop.stop()
