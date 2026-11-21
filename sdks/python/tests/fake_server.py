"""A scriptable stand-in for the Checkgate server's SDK endpoints (``/stream``
and ``/flags/snapshot``), speaking the same wire protocol (chunked SSE, v2
snapshots), for the sync state machine's unit tests. Standard library only.

The cross-SDK fault suite (sdks/conformance/fault-server.mjs) covers random
fault mixes; this one makes each case deterministic.
"""

from __future__ import annotations

import json
import queue
import socket
import threading
import time
from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Optional

ENV_ID = "00000000-0000-4000-8000-00000000c0f1"
_KILL = object()
_STALL = object()


def frame(event: str, data: Any) -> str:
    return f"event: {event}\ndata: {data if isinstance(data, str) else json.dumps(data)}\n\n"


def flag(key: str, enabled: bool = True, **extra: Any) -> dict[str, Any]:
    return {"key": key, "is_enabled": enabled, "rollout_percentage": None, "description": None,
            "rules": [], **extra}


class _Stream:
    def __init__(self) -> None:
        self.q: "queue.Queue[Any]" = queue.Queue()


class FakeServer:
    def __init__(self, *, heartbeat_ms: Optional[int] = None, sdk_key: str = "sk_test",
                 legacy_snapshot: bool = False) -> None:
        self.heartbeat_ms = heartbeat_ms
        self.sdk_key = sdk_key
        self.accepted_key: Optional[str] = sdk_key
        self.legacy_snapshot = legacy_snapshot
        self.lock = threading.Lock()
        self.revision = 0
        self.flags: dict[str, tuple[dict[str, Any], int]] = {}
        self.tombstones: dict[str, int] = {}
        self.streams: set[_Stream] = set()
        # 'pass' | 'refuse' (socket closed) | 'unavailable' (503) | 'silent'
        # (stream headers, then nothing)
        self.stream_mode = "pass"
        self.snapshot_mode = "pass"
        self.snapshot_delay = 0.0
        self.hold_next_stream = 0.0
        self.held_stream_opened = threading.Event()
        self.cut_next_replay = False
        self.counters: Counter[str] = Counter()
        self.stream_opened_at: list[float] = []

    # -- lifecycle -------------------------------------------------------------

    def start(self) -> str:
        server = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args: Any) -> None:
                pass

            def do_GET(self) -> None:
                path = self.path.split("?", 1)[0]
                if path == "/stream":
                    server._stream(self)
                elif path == "/flags/snapshot":
                    server._snapshot(self)
                else:
                    self.send_response(404)
                    self.send_header("Content-Length", "0")
                    self.end_headers()

        class Server(ThreadingHTTPServer):
            def handle_error(self, request: Any, client_address: Any) -> None:
                pass  # clients dropping connections is what these tests do

        self.httpd = Server(("127.0.0.1", 0), Handler)
        self.httpd.daemon_threads = True
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()
        return f"http://127.0.0.1:{self.httpd.server_address[1]}"

    def stop(self) -> None:
        self.sever_all()
        self.httpd.shutdown()
        self.httpd.server_close()

    # -- authoritative state ---------------------------------------------------

    def upsert(self, f: dict[str, Any], broadcast: bool = True) -> int:
        with self.lock:
            self.revision += 1
            rev = self.revision
            self.flags[f["key"]] = (f, rev)
            self.tombstones.pop(f["key"], None)
            if broadcast:
                self._broadcast(frame("update", {"type": "UPSERT", "env_id": ENV_ID, "flag": f, "revision": rev}))
        return rev

    def delete(self, key: str, broadcast: bool = True) -> int:
        with self.lock:
            self.revision += 1
            rev = self.revision
            self.flags.pop(key, None)
            self.tombstones[key] = rev
            if broadcast:
                self._broadcast(frame("update", {"type": "DELETE", "env_id": ENV_ID, "key": key, "revision": rev}))
        return rev

    def send(self, text: str) -> None:
        """Raw bytes to every open stream (e.g. a stale or malformed event)."""
        with self.lock:
            self._broadcast(text)

    def _broadcast(self, text: str) -> None:
        for s in self.streams:
            s.q.put(text)

    # -- faults ----------------------------------------------------------------

    def sever_all(self) -> None:
        with self.lock:
            for s in self.streams:
                s.q.put(_KILL)

    def stall_all(self) -> None:
        with self.lock:
            for s in self.streams:
                s.q.put(_STALL)

    def open_streams(self) -> int:
        with self.lock:
            return len(self.streams)

    # -- handlers --------------------------------------------------------------

    def _gate(self, h: BaseHTTPRequestHandler, mode: str) -> bool:
        if mode == "refuse":
            h.close_connection = True
            try:
                h.connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            return False
        if mode == "unavailable":
            h.send_response(503)
            h.send_header("Content-Length", "4")
            h.end_headers()
            h.wfile.write(b"busy")
            return False
        auth = h.headers.get("Authorization", "")
        key = auth[7:] if auth.startswith("Bearer ") else None
        if self.accepted_key is None or key != self.accepted_key:
            self.counters["unauthorized"] += 1
            h.send_response(401)
            h.send_header("Content-Length", "0")
            h.end_headers()
            return False
        return True

    def _snapshot(self, h: BaseHTTPRequestHandler) -> None:
        self.counters["snapshot_requests"] += 1
        if not self._gate(h, self.snapshot_mode):
            return
        with self.lock:
            flags = [f for f, _ in self.flags.values()]
            if "v=2" in h.path and not self.legacy_snapshot:
                body: Any = {"v": 2, "flags": flags,
                             "revisions": {k: r for k, (_, r) in self.flags.items()},
                             "tombstones": dict(self.tombstones)}
            else:
                body = flags
        raw = json.dumps(body).encode()
        if self.snapshot_delay:
            time.sleep(self.snapshot_delay)
        h.send_response(200)
        h.send_header("Content-Type", "application/json")
        h.send_header("X-Checkgate-Environment-Id", ENV_ID)
        h.send_header("Content-Length", str(len(raw)))
        h.end_headers()
        h.wfile.write(raw)

    def _stream(self, h: BaseHTTPRequestHandler) -> None:
        self.counters["stream_requests"] += 1
        if not self._gate(h, self.stream_mode if self.stream_mode != "silent" else "pass"):
            return
        h.close_connection = True
        h.send_response(200)
        h.send_header("Content-Type", "text/event-stream")
        h.send_header("Cache-Control", "no-cache")
        h.send_header("Transfer-Encoding", "chunked")
        h.end_headers()
        h.wfile.flush()

        def write(text: str) -> None:
            data = text.encode()
            h.wfile.write(f"{len(data):x}\r\n".encode() + data + b"\r\n")
            h.wfile.flush()

        stream = _Stream()
        with self.lock:
            self.streams.add(stream)
            self.counters["streams_opened"] += 1
            self.stream_opened_at.append(time.monotonic())
            silent = self.stream_mode == "silent"
            hold, self.hold_next_stream = self.hold_next_stream, 0.0
            cut, self.cut_next_replay = self.cut_next_replay, False
            # The replay is read now, when the stream opens, as the server does.
            hello = {"environment_id": ENV_ID}
            if self.heartbeat_ms:
                hello["heartbeat_ms"] = self.heartbeat_ms
            replay = [frame("connected", hello)]
            for key in sorted(self.flags):
                f, rev = self.flags[key]
                replay.append(frame("update", {"type": "UPSERT", "env_id": ENV_ID, "flag": f, "revision": rev}))
            for key, rev in self.tombstones.items():
                replay.append(frame("update", {"type": "DELETE", "env_id": ENV_ID, "key": key, "revision": rev}))
            replay.append(frame("ready", str(len(self.flags))))
        try:
            if hold:
                self.held_stream_opened.set()
                time.sleep(hold)
            if not silent:
                if cut:
                    for text in replay[: len(replay) // 2]:
                        write(text)
                    partial = replay[len(replay) // 2]
                    write(partial[: len(partial) // 2])
                    return
                for text in replay:
                    write(text)
            stalled = False
            last = time.monotonic()
            while True:
                try:
                    item = stream.q.get(timeout=0.02)
                except queue.Empty:
                    if _peer_closed(h.connection):
                        return
                    now = time.monotonic()
                    if self.heartbeat_ms and not stalled and not silent and now - last >= self.heartbeat_ms / 1000.0:
                        write(frame("heartbeat", "{}"))
                        self.counters["heartbeats"] += 1
                        last = now
                    continue
                if item is _KILL:
                    return
                if item is _STALL:
                    stalled = True
                    continue
                if not stalled and not silent:
                    write(item)
                    last = time.monotonic()
        except OSError:
            return
        finally:
            with self.lock:
                self.streams.discard(stream)
            try:
                h.connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass


def _peer_closed(conn: socket.socket) -> bool:
    try:
        return conn.recv(1, socket.MSG_PEEK | socket.MSG_DONTWAIT) == b""
    except (BlockingIOError, InterruptedError):
        return False
    except OSError:
        return True


def wait_until(probe: Any, timeout: float = 5.0, interval: float = 0.01) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if probe():
            return True
        time.sleep(interval)
    return bool(probe())
