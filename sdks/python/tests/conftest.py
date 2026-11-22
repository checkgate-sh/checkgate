from __future__ import annotations

import os
import sys
from pathlib import Path
from typing import Any, Callable, Iterator

import pytest

sys.path.insert(0, str(Path(__file__).parent))

from fake_server import FakeServer  # noqa: E402

from checkgate import CheckgateClient  # noqa: E402

REPO = Path(__file__).resolve().parents[3]
VECTORS = REPO / "sdks" / "conformance" / "vectors" / "evaluation.json"

#: Production rules, test-sized constants (as sdks/conformance/scenario.mjs).
FAST = dict(
    reconnect_delay_ms=20,
    max_reconnect_delay_ms=200,
    poll_fallback_threshold=3,
    poll_interval_ms=100,
    connect_timeout_ms=500,
)


@pytest.fixture
def server() -> Iterator[FakeServer]:
    s = FakeServer(heartbeat_ms=100)
    s.url = s.start()
    yield s
    s.stop()


@pytest.fixture
def make_client() -> Iterator[Callable[..., CheckgateClient]]:
    clients: list[CheckgateClient] = []

    def make(url: str, sdk_key: str = "sk_test", **options: Any) -> CheckgateClient:
        c = CheckgateClient(url, sdk_key, **{**FAST, **options})
        clients.append(c)
        return c

    yield make
    for c in clients:
        c.close()


def pytest_report_header(config: Any) -> str:
    import checkgate
    from checkgate import _core

    return f"checkgate {checkgate.__version__} (engine {_core.CORE_VERSION}) from {os.path.dirname(checkgate.__file__)}"
