"""A minimal single-threaded task loop with timers, on one daemon thread.

The client's sync state machine is a port of the Node SDK, whose code runs on
an event loop: every callback runs alone, and timers can be cancelled. This
loop gives the Python port the same model without asyncio, so it never needs
(or blocks) the application's event loop. Blocking I/O happens on separate
worker threads that post their results here.
"""

from __future__ import annotations

import heapq
import itertools
import logging
import threading
import time
from collections import deque
from typing import Callable, Optional

log = logging.getLogger("checkgate")


class Timer:
    """A scheduled call. ``cancel()`` is final once it returns, provided the
    caller and the timer's callback serialize on the same lock (the client's
    state lock): the callback checks ``cancelled`` under it."""

    __slots__ = ("fn", "cancelled")

    def __init__(self, fn: Callable[[], None]) -> None:
        self.fn = fn
        self.cancelled = False

    def cancel(self) -> None:
        self.cancelled = True


class Loop:
    def __init__(self, name: str) -> None:
        self._cv = threading.Condition(threading.Lock())
        self._ready: deque[Callable[[], None]] = deque()
        self._timers: list[tuple[float, int, Timer]] = []
        self._seq = itertools.count()
        self._stopped = False
        self._thread = threading.Thread(target=self._run, name=name, daemon=True)
        self._started = False

    def start(self) -> None:
        with self._cv:
            if self._started or self._stopped:
                return
            self._started = True
        self._thread.start()

    def in_loop(self) -> bool:
        return threading.current_thread() is self._thread

    def call_soon(self, fn: Callable[[], None]) -> None:
        with self._cv:
            if self._stopped:
                return
            self._ready.append(fn)
            self._cv.notify()

    def call_later(self, delay: float, fn: Callable[[], None]) -> Timer:
        timer = Timer(fn)
        with self._cv:
            if not self._stopped:
                heapq.heappush(self._timers, (time.monotonic() + max(0.0, delay), next(self._seq), timer))
                self._cv.notify()
        return timer

    def stop(self) -> None:
        with self._cv:
            self._stopped = True
            self._ready.clear()
            self._timers.clear()
            self._cv.notify()

    def join(self, timeout: Optional[float]) -> None:
        if self._started and not self.in_loop():
            self._thread.join(timeout)

    def _run(self) -> None:
        while True:
            with self._cv:
                while True:
                    if self._stopped:
                        return
                    now = time.monotonic()
                    while self._timers and self._timers[0][0] <= now:
                        _, _, timer = heapq.heappop(self._timers)
                        if not timer.cancelled:
                            self._ready.append(timer.fn)
                    if self._ready:
                        fn = self._ready.popleft()
                        break
                    # Bounded: Condition.wait raises OverflowError beyond
                    # TIMEOUT_MAX, which would end this thread for good.
                    self._cv.wait(min(self._timers[0][0] - now, threading.TIMEOUT_MAX) if self._timers else None)
            try:
                fn()
            except Exception:  # a bug in a task must not end syncing
                log.exception("[Checkgate] internal task failed")
