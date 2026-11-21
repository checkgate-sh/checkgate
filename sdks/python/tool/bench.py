"""Latency of ``CheckgateClient.is_enabled`` (and the raw engine call), with
no attributes and with 5 attributes. Run with a release build installed:

    (cd sdks/python && maturin develop --release && .venv/bin/python tool/bench.py)

Reports ns per call: the minimum and median over repeats of timeit loops
(the minimum is the least disturbed by other load on the machine).
"""

from __future__ import annotations

import json
import os
import platform
import statistics
import sys
import threading
import time
import timeit

from checkgate import CheckgateClient
from checkgate._core import CORE_VERSION

NUMBER = int(os.environ.get("BENCH_NUMBER", "200000"))
REPEAT = int(os.environ.get("BENCH_REPEAT", "7"))

FLAGS = [
    {"key": "simple", "is_enabled": True, "rollout_percentage": None, "description": None, "rules": []},
    {"key": "rollout", "is_enabled": True, "rollout_percentage": 50, "description": None, "rules": []},
    {"key": "targeted", "is_enabled": True, "rollout_percentage": 0, "description": None,
     "rules": [{"attribute": "plan", "operator": "equals", "values": ["pro", "enterprise"]},
               {"attribute": "country", "operator": "equals", "values": ["PH"]}]},
]
ATTRS5 = {"plan": "pro", "country": "PH", "email": "a@example.com", "beta": "true", "app_version": "4.2.0"}


def measure(stmt: str, env: dict[str, object]) -> tuple[float, float]:
    times = timeit.repeat(stmt, globals=env, number=NUMBER, repeat=REPEAT)
    per = [t / NUMBER * 1e9 for t in times]
    return min(per), statistics.median(per)


def main() -> None:
    client = CheckgateClient("http://127.0.0.1:9")  # never connected: primed directly
    client._engine.apply([(f["key"], json.dumps(f)) for f in FLAGS], [])
    client._hydrated = True
    engine = client._engine
    env = {"c": client, "e": engine, "A": ATTRS5}
    rows = [
        ("client.is_enabled, no rules, no attributes", 'c.is_enabled("simple", "user-123")'),
        ("client.is_enabled, 50% rollout, no attributes", 'c.is_enabled("rollout", "user-123")'),
        ("client.is_enabled, no rules, 5 attributes", 'c.is_enabled("simple", "user-123", A)'),
        ("client.is_enabled, 2 rules, 5 attributes", 'c.is_enabled("targeted", "user-123", A)'),
        ("engine.is_enabled, no rules, no attributes", 'e.is_enabled("simple", "user-123")'),
        ("engine.is_enabled, 2 rules, 5 attributes", 'e.is_enabled("targeted", "user-123", A)'),
        ("client.get_value, no rules", 'c.get_value("simple", "user-123")'),
        ("client.get_details, 2 rules, 5 attributes", 'c.get_details("targeted", "user-123", A)'),
        ("baseline: empty Python method call", "c.is_ready()"),
    ]
    print(f"python {platform.python_version()} ({platform.python_implementation()}), engine {CORE_VERSION}, "
          f"{platform.platform()}, {platform.machine()}, cpus={os.cpu_count()}")
    try:
        load = os.getloadavg()
        print(f"load average at start: {load[0]:.1f} {load[1]:.1f} {load[2]:.1f}")
    except OSError:
        pass
    print(f"timeit: number={NUMBER} repeat={REPEAT}")
    for label, stmt in rows:
        lo, med = measure(stmt, env)
        print(f"  {label:<50} min {lo:7.0f} ns   median {med:7.0f} ns")

    # Throughput with several threads (the GIL is released inside the engine).
    for threads in (1, 2, 4):
        n = 200_000 // threads
        def work() -> None:
            f = client.is_enabled
            for _ in range(n):
                f("targeted", "user-123", ATTRS5)
        ts = [threading.Thread(target=work) for _ in range(threads)]
        start = time.perf_counter()
        for t in ts:
            t.start()
        for t in ts:
            t.join()
        elapsed = time.perf_counter() - start
        print(f"  {threads} thread(s), 2 rules + 5 attributes: {n * threads / elapsed / 1e6:.2f} M evaluations/s total")
    client.close()


if __name__ == "__main__":
    sys.exit(main())
