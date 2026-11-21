"""Fork safety, and client-level evaluation while the sync thread applies
snapshots and deltas."""

from __future__ import annotations

import os
import sys
import threading
import time
import warnings
from typing import Any, Callable

import pytest
from fake_server import FakeServer, flag, wait_until

from checkgate import CheckgateClient, CheckgateForkError

Make = Callable[..., CheckgateClient]


@pytest.mark.skipif(not hasattr(os, "fork"), reason="os.fork() is POSIX-only")
def test_client_created_before_fork_raises_in_the_child(server: FakeServer, make_client: Make) -> None:
    server.upsert(flag("a"))
    c = make_client(server.url)
    c.connect(timeout=5)
    r, w = os.pipe()
    with warnings.catch_warnings():
        # Python 3.12+ warns that forking a multi-threaded process may
        # deadlock: exactly the hazard this guard exists for.
        warnings.simplefilter("ignore", DeprecationWarning)
        pid = os.fork()
    if pid == 0:  # child: report, never return into pytest
        code = 0
        try:
            outcomes = []
            for call in (lambda: c.is_enabled("a", "u"), lambda: c.get_value("a", "u"), c.get_health,
                         lambda: c.connect(timeout=1), c.get_status, lambda: c.get_details("a", "u")):
                try:
                    call()
                    outcomes.append("returned")
                except CheckgateForkError:
                    outcomes.append("fork-error")
            c.close()  # allowed: marks it closed without touching inherited locks
            fresh = CheckgateClient(server.url, "sk_test", connect_timeout_ms=2000)
            fresh.connect(timeout=5)
            outcomes.append("fresh-ok" if fresh.is_enabled("a", "u") else "fresh-bad")
            fresh.close()
            os.write(w, ",".join(outcomes).encode())
        except BaseException as err:  # pragma: no cover - reported to the parent
            os.write(w, f"crash:{err!r}".encode())
            code = 1
        finally:
            os._exit(code)
    os.close(w)
    _, status = os.waitpid(pid, 0)
    out = os.read(r, 4096).decode()
    os.close(r)
    assert os.waitstatus_to_exitcode(status) == 0, out
    assert out == ",".join(["fork-error"] * 6 + ["fresh-ok"]), out
    # The parent's client is unaffected.
    assert c.is_enabled("a", "u") is True
    assert c.get_status()["state"] == "streaming"


def test_evaluations_during_sync_always_see_a_complete_set(server: FakeServer, make_client: Make) -> None:
    """Version k: `child` requires `parent` == "v{k}" and `parent` is "v{k}".
    Versions are delivered by full replays (reconnects) and polls; any torn
    set makes `child` false. Deltas are not atomic across two flags by
    design (each is one server commit), so they are sent for unrelated
    flags only."""

    def publish(k: int, broadcast: bool) -> None:
        # Both flags change in one step on the server: no delta between.
        server.upsert(flag("parent", flag_type="string", default_value=f"v{k}"), broadcast=broadcast)
        server.upsert(flag("child", prerequisites=[{"flag_key": "parent", "required_value": f"v{k}"}]),
                      broadcast=broadcast)

    publish(0, True)
    for i in range(30):
        server.upsert(flag(f"pad{i}"))
    c = make_client(server.url)
    c.connect(timeout=5)
    stop = threading.Event()
    failures: list[str] = []
    counts = [0] * 6

    def reader(slot: int) -> None:
        while not stop.is_set():
            try:
                if not c.is_enabled("child", f"user-{slot}"):
                    failures.append("torn")
                v = c.get_value("parent", "u")
                if not (isinstance(v, str) and v.startswith("v")):
                    failures.append(f"value {v!r}")
                counts[slot] += 1
            except Exception as err:  # pragma: no cover
                failures.append(repr(err))

    readers = [threading.Thread(target=reader, args=(i,)) for i in range(6)]
    for t in readers:
        t.start()
    end = time.monotonic() + 3
    k = 0
    while time.monotonic() < end:
        k += 1
        publish(k, broadcast=False)
        server.upsert(flag(f"pad{k % 30}", description=str(k)))  # a delta to an unrelated flag
        server.sever_all()  # the next replay carries version k
        want = f"v{k}"
        wait_until(lambda: c.get_value("parent", "u") == want, 3)
        if k % 4 == 0:
            server.stream_mode = "unavailable"
            server.sever_all()
            wait_until(lambda: c.is_polling(), 3)
            k += 1
            publish(k, broadcast=False)
            want = f"v{k}"
            wait_until(lambda: c.get_value("parent", "u") == want, 3)  # delivered by a poll
            server.stream_mode = "pass"
    stop.set()
    for t in readers:
        t.join()
    assert failures == []
    assert k >= 4 and all(n > 0 for n in counts)
    print(f"versions={k} evaluations={sum(counts)}", file=sys.stderr)
