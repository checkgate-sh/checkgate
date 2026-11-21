"""An incremental Server-Sent Events parser (the WHATWG event-stream format).

Fed raw bytes as they arrive, in chunks of any size (a frame may be split
anywhere, down to one byte per chunk); returns each event once its frame is
complete. An event whose frame never completes (the connection dropped
mid-frame) is never returned.
"""

from __future__ import annotations

import re

_EOL = re.compile(rb"\r\n|\r|\n")
_BOM = b"\xef\xbb\xbf"

#: Upper bound on one unterminated line or one event's data: a stream that
#: exceeds it is treated as broken rather than buffered without limit.
MAX_EVENT_BYTES = 16 * 1024 * 1024


class SSEError(ValueError):
    """The stream broke the format's limits."""


class SSEParser:
    """Turns event-stream bytes into ``(event, data)`` pairs."""

    def __init__(self, max_event_bytes: int = MAX_EVENT_BYTES) -> None:
        self._max = max_event_bytes
        self._buf = b""
        self._first = True
        self._skip_lf = False
        self._event = ""
        self._data: list[str] = []
        self._data_bytes = 0

    def feed(self, chunk: bytes) -> list[tuple[str, str]]:
        events: list[tuple[str, str]] = []
        if self._buf and not self._skip_lf and b"\n" not in chunk and b"\r" not in chunk:
            # Mid-line (e.g. a stream fragmented into tiny writes): only
            # buffer, never rescan what is already buffered.
            self._buf += chunk
            if len(self._buf) > self._max:
                raise SSEError("event-stream line exceeds the size limit")
            return events
        buf = self._buf + chunk if self._buf else chunk
        pos = 0
        if self._skip_lf and buf[:1] == b"\n":
            pos = 1
        if buf:
            self._skip_lf = False
        while True:
            match = _EOL.search(buf, pos)
            if match is None:
                break
            line = buf[pos : match.start()]
            # A lone CR at the end of the buffer may be the first half of CRLF.
            if match.group() == b"\r" and match.end() == len(buf):
                self._skip_lf = True
            self._line(line, events)
            pos = match.end()
        self._buf = buf[pos:]
        if len(self._buf) > self._max:
            raise SSEError("event-stream line exceeds the size limit")
        return events

    def _line(self, raw: bytes, events: list[tuple[str, str]]) -> None:
        if self._first:
            self._first = False
            if raw.startswith(_BOM):
                raw = raw[len(_BOM) :]
        if not raw:
            # Blank line: dispatch. An event with no data is not dispatched.
            if self._data:
                events.append((self._event or "message", "\n".join(self._data)))
            self._event = ""
            self._data = []
            self._data_bytes = 0
            return
        if raw[:1] == b":":
            return  # comment (pre-0.1.31 servers send keep-alives this way)
        line = raw.decode("utf-8", errors="replace")
        field, colon, value = line.partition(":")
        if colon and value.startswith(" "):
            value = value[1:]
        if field == "event":
            self._event = value
        elif field == "data":
            self._data_bytes += len(raw)
            if self._data_bytes > self._max:
                raise SSEError("event-stream event exceeds the size limit")
            self._data.append(value)
        # "id" and "retry" are not used: the client manages its own backoff
        # and every (re)connect receives a full replay.
