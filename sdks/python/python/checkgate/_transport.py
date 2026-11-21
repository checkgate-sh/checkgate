"""Blocking HTTP for the sync loop: one worker thread per stream attempt or
poll, reporting back through a callback. Standard library only.

A request is aborted from another thread by shutting its socket down, which
makes the worker's blocking read return; the worker then closes the socket
itself (closing a descriptor another thread is blocked on is not safe).
"""

from __future__ import annotations

import http.client
import socket
import threading
from typing import Any, Callable, Optional
from urllib.parse import urlsplit

from ._sse import SSEError, SSEParser

#: Most a snapshot body may be; a larger one is rejected, not buffered.
MAX_SNAPSHOT_BYTES = 64 * 1024 * 1024


class Endpoint:
    """The server's base URL, split once."""

    def __init__(self, server_url: str) -> None:
        parts = urlsplit(server_url.rstrip("/"))
        if parts.scheme not in ("http", "https") or not parts.hostname:
            raise ValueError("server_url must be an http:// or https:// URL")
        self.https = parts.scheme == "https"
        self.host = parts.hostname
        self.port = parts.port
        self.base_path = parts.path

    def connection(self, timeout: Optional[float]) -> http.client.HTTPConnection:
        if self.https:
            return http.client.HTTPSConnection(self.host, self.port, timeout=timeout)
        return http.client.HTTPConnection(self.host, self.port, timeout=timeout)


def _shutdown(sock: Optional[socket.socket]) -> None:
    if sock is None:
        return
    try:
        # The plain socket's shutdown, also for an SSL socket: unblocks a
        # reader without tearing down TLS state another thread is using.
        socket.socket.shutdown(sock, socket.SHUT_RDWR)
    except OSError:
        pass


class Request:
    """One abortable HTTP exchange, run on its own daemon thread."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._sock: Optional[socket.socket] = None
        self.cancelled = False

    def cancel(self) -> None:
        with self._lock:
            self.cancelled = True
            sock = self._sock
        _shutdown(sock)

    def _attach(self, conn: http.client.HTTPConnection) -> bool:
        with self._lock:
            if self.cancelled:
                return False
            self._sock = conn.sock
            return True


def classify(err: BaseException) -> str:
    """The health ``last_error.kind`` of a transport failure."""
    if isinstance(err, (socket.timeout, TimeoutError)):
        return "timeout"
    if isinstance(err, (ValueError, SSEError)):
        return "parse"
    return "network"


class StreamRequest(Request):
    """``GET /stream``. Reports through ``report(kind, *args)``:

    - ``("event", name, data)`` for each complete event, in order;
    - ``("status", code)`` for a non-200 answer;
    - ``("error", kind, message)`` when the stream fails or ends.

    Once cancelled it reports nothing more.
    """

    def __init__(self, endpoint: Endpoint, headers: dict[str, str], connect_timeout: float,
                 report: Callable[..., None]) -> None:
        super().__init__()
        self._endpoint = endpoint
        self._headers = headers
        self._connect_timeout = connect_timeout
        self._report = report

    def run(self) -> None:
        conn = None
        try:
            conn = self._endpoint.connection(self._connect_timeout)
            conn.connect()
            if not self._attach(conn):
                return
            # From here the client's watchdog owns every deadline (connect and
            # idle) and aborts by shutting the socket down.
            if conn.sock is not None:
                conn.sock.settimeout(None)
            conn.request("GET", self._endpoint.base_path + "/stream", headers={
                **self._headers, "Accept": "text/event-stream", "Cache-Control": "no-cache",
            })
            resp = conn.getresponse()
            if resp.status != 200:
                if not self.cancelled:
                    self._report("status", resp.status)
                return
            ctype = resp.getheader("Content-Type", "") or ""
            if not ctype.lower().startswith("text/event-stream"):
                if not self.cancelled:
                    self._report("error", "http", f"unexpected Content-Type {ctype!r}")
                return
            parser = SSEParser()
            while not self.cancelled:
                chunk = resp.read1(65536)
                if not chunk:
                    break
                for name, data in parser.feed(chunk):
                    if self.cancelled:
                        return
                    self._report("event", name, data)
            if not self.cancelled:
                self._report("error", "network", "stream ended")
        except Exception as err:  # every failure ends this attempt alike
            if not self.cancelled:
                self._report("error", classify(err), f"{type(err).__name__}: {err}")
        finally:
            if conn is not None:
                conn.close()


class PollRequest(Request):
    """``GET /flags/snapshot?v=2``. Reports once through ``report``:
    ``("ok", body, environment_id)``, ``("status", code)`` or
    ``("error", kind, message)``; nothing once cancelled."""

    def __init__(self, endpoint: Endpoint, headers: dict[str, str], timeout: float,
                 report: Callable[..., None], parse: Callable[[str], Any]) -> None:
        super().__init__()
        self._endpoint = endpoint
        self._headers = headers
        self._timeout = timeout
        self._report = report
        self._parse = parse

    def run(self) -> None:
        conn = None
        try:
            conn = self._endpoint.connection(self._timeout)
            conn.connect()
            if not self._attach(conn):
                return
            conn.request("GET", self._endpoint.base_path + "/flags/snapshot?v=2", headers={
                **self._headers, "Accept": "application/json",
            })
            resp = conn.getresponse()
            if not 200 <= resp.status < 300:
                if not self.cancelled:
                    self._report("status", resp.status)
                return
            raw = resp.read(MAX_SNAPSHOT_BYTES + 1)
            if len(raw) > MAX_SNAPSHOT_BYTES:
                raise ValueError("snapshot exceeds the size limit")
            body = self._parse(raw.decode("utf-8"))
            env_id = resp.getheader("X-Checkgate-Environment-Id")
            if not self.cancelled:
                self._report("ok", body, env_id)
        except Exception as err:
            if not self.cancelled:
                self._report("error", classify(err), f"{type(err).__name__}: {err}")
        finally:
            if conn is not None:
                conn.close()
