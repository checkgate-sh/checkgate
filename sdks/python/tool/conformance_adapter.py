"""Drives one CheckgateClient for the shared fault-injection suite
(sdks/conformance/python.mjs runs this and sends it the scenarios). The same
JSON-lines protocol as sdks/flutter/dart/tool/conformance_adapter.dart: one
JSON object per line on stdin, one reply per line on stdout.

  {"id":1,"cmd":"create","url":"…","key":"…","timing":{…}}
  {"id":2,"cmd":"connect"}  -> {"id":2,"ok":true}  or  {"id":2,"ok":false,"error":"…"}
  {"id":3,"cmd":"flags"}    -> {"id":3,"flags":{key: flag}}
  {"id":4,"cmd":"evaluate","key":"…","user":"…","attrs":{…}} -> {"id":4,"value":true|false}
  {"id":9,"cmd":"value","key":"…","user":"…"} -> {"id":9,"value":<resolved value or null>}
  {"id":5,"cmd":"status"}   -> {"id":5,"state":"…"}
  {"id":7,"cmd":"sample","users":[…]} -> flags and evaluations at one instant
  {"id":6,"cmd":"close"}
  {"id":8,"cmd":"exit"}

The real Rust engine always evaluates (the installed checkgate package).
SDK log output is silenced unless CONFORMANCE_VERBOSE is set (it would go to
stderr, never stdout).
"""

from __future__ import annotations

import json
import logging
import os
import sys
import threading
from typing import Any, Optional

from checkgate import CheckgateClient

DEFAULT_ATTRS = {"plan": "pro"}
_out = threading.Lock()


def reply(msg: dict[str, Any]) -> None:
    line = json.dumps(msg)
    with _out:
        sys.stdout.write(line + "\n")
        sys.stdout.flush()


def main() -> None:
    if os.environ.get("CONFORMANCE_VERBOSE"):
        logging.basicConfig(stream=sys.stderr, level=logging.DEBUG,
                            format="%(relativeCreated)7.0f ms %(threadName)s %(message)s")
    else:
        logging.getLogger("checkgate").setLevel(logging.CRITICAL + 1)
    client: Optional[CheckgateClient] = None
    for line in sys.stdin:
        if not line.strip():
            continue
        msg = json.loads(line)
        mid = msg.get("id")
        cmd = msg.get("cmd")
        if cmd == "create":
            t = msg["timing"]
            client = CheckgateClient(
                msg["url"], msg["key"],
                reconnect_delay_ms=t["reconnectDelayMs"],
                max_reconnect_delay_ms=t["maxReconnectDelayMs"],
                poll_fallback_threshold=t["pollFallbackThreshold"],
                poll_interval_ms=t["pollIntervalMs"],
                connect_timeout_ms=t["connectTimeoutMs"],
            )
            reply({"id": mid, "ok": True, "engine": True})
        elif cmd == "connect":
            assert client is not None
            c = client

            def run(mid: Any = mid, c: CheckgateClient = c) -> None:
                try:
                    c.connect()
                    reply({"id": mid, "ok": True})
                except Exception as err:
                    reply({"id": mid, "ok": False, "error": f"{type(err).__name__}: {err}"})

            threading.Thread(target=run, daemon=True).start()
        elif cmd == "flags":
            assert client is not None
            reply({"id": mid, "flags": client._debug_flags()})
        elif cmd == "evaluate":
            assert client is not None
            attrs = msg.get("attrs")
            reply({"id": mid, "value": client.is_enabled(msg["key"], msg["user"], attrs if attrs is not None else DEFAULT_ATTRS)})
        elif cmd == "value":
            assert client is not None
            reply({"id": mid, "value": client.get_value(msg["key"], msg["user"], msg.get("attrs") or {}, None)})
        elif cmd == "sample":
            assert client is not None
            flags, evals = client._debug_sample(list(msg["users"]), DEFAULT_ATTRS)
            reply({"id": mid, "flags": flags, "evals": evals})
        elif cmd == "status":
            assert client is not None
            reply({"id": mid, "state": client.get_status()["state"]})
        elif cmd == "health":
            assert client is not None
            reply({"id": mid, "health": client.get_health()})
        elif cmd == "close":
            if client is not None:
                client.close()
            reply({"id": mid, "ok": True})
        elif cmd == "exit":
            reply({"id": mid, "ok": True})
            return


if __name__ == "__main__":
    main()
