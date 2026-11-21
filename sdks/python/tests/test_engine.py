"""The Rust engine as Python sees it: attribute decoding, value types, errors,
and evaluation under concurrent writes."""

from __future__ import annotations

import json
import threading
import time
from collections import UserDict
from typing import Any

import pytest

from checkgate._core import Engine


def flag(key: str, **extra: Any) -> str:
    return json.dumps({"key": key, "is_enabled": True, "rollout_percentage": None, "description": None,
                       "rules": [], **extra})


PAID_ONLY = flag("paid-only", rollout_percentage=0,
                 rules=[{"attribute": "plan", "operator": "not_equals", "values": ["free"]}])


def test_string_attributes_kept_non_strings_skipped() -> None:
    e = Engine()
    assert e.upsert_flag_json(PAID_ONLY)
    # "plan" is a string: the excluded user stays excluded even though another
    # attribute is not a string (checkgate_core::attributes semantics).
    assert e.is_enabled("paid-only", "u", {"plan": "free", "age": 30, "beta": True, "x": None}) is False
    # A non-string "plan" is skipped (absent), so not_equals matches.
    assert e.is_enabled("paid-only", "u", {"plan": 7}) is True
    assert e.is_enabled("paid-only", "u", {"plan": "pro"}) is True


def test_str_subclass_counts_as_string() -> None:
    class Name(str):
        pass

    e = Engine()
    e.upsert_flag_json(PAID_ONLY)
    assert e.is_enabled("paid-only", "u", {"plan": Name("free")}) is False


def test_attributes_through_prerequisites() -> None:
    e = Engine()
    e.upsert_flag_json(flag("base", rollout_percentage=0,
                            rules=[{"attribute": "country", "operator": "equals", "values": ["PH"]}]))
    e.upsert_flag_json(flag("child", prerequisites=[{"flag_key": "base"}]))
    assert e.is_enabled("child", "u", {"country": "PH"}) is True
    assert e.is_enabled("child", "u", {"country": "US"}) is False
    d = e.explain("child", "u", {"country": "US"})
    assert d["reason"] == "PREREQUISITE_FAILED"
    assert d["prerequisites"][0]["flag_key"] == "base" and d["prerequisites"][0]["outcome"] == "unsatisfied"


def test_value_types() -> None:
    e = Engine()
    e.upsert_flag_json(flag("s", flag_type="string", default_value="blue"))
    e.upsert_flag_json(flag("i", flag_type="integer", default_value=42))
    e.upsert_flag_json(flag("j", flag_type="json", default_value={"a": [1, 2.5, None, True]}))
    assert e.get_variant("s", "u") == (True, "blue")
    enabled, value = e.get_variant("i", "u")
    assert value == 42 and type(value) is int
    assert e.get_variant("j", "u") == (True, {"a": [1, 2.5, None, True]})
    assert e.get_variant("missing", "u") is None
    assert e.explain("missing", "u") is None


def test_wrong_argument_types_raise_type_error() -> None:
    e = Engine()
    e.upsert_flag_json(PAID_ONLY)
    with pytest.raises(TypeError):
        e.is_enabled("paid-only", 123)  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        e.is_enabled("paid-only", "u", ["plan"])  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        e.upsert_flag_json(b"{}")  # type: ignore[arg-type]


def test_mapping_attributes_via_client_conversion() -> None:
    # The engine takes dicts; the client converts other mappings.
    from checkgate._client import _as_dict

    assert _as_dict(UserDict({"plan": "pro"})) == {"plan": "pro"}
    d = {"plan": "pro"}
    assert _as_dict(d) is d


def test_apply_is_atomic_and_fails_closed() -> None:
    e = Engine()
    e.apply([("a", flag("a")), ("b", flag("b"))], [])
    rejected = e.apply([("a", '{"key":"a","is_enabled":"yes"}'), ("c", flag("c")), ("d", flag("x"))], ["b"])
    assert rejected == ["a", "d"]  # invalid, and key mismatch
    assert e.flag_keys() == ["c"]
    assert e.delete_flag("c") is True and e.delete_flag("c") is False
    e.upsert_flag_json(flag("z"))
    e.clear()
    assert len(e) == 0


def test_concurrent_evaluation_never_sees_a_mixed_set() -> None:
    """Version k of the set: `child` requires `parent` to resolve to "v{k}",
    and `parent` resolves to "v{k}". Any mix of two versions fails the
    prerequisite, so a single False proves a torn read."""
    e = Engine()

    def version(k: int) -> list[tuple[str, str]]:
        return [
            ("parent", flag("parent", flag_type="string", default_value=f"v{k}")),
            ("child", flag("child", prerequisites=[{"flag_key": "parent", "required_value": f"v{k}"}])),
        ] + [(f"pad{i}", flag(f"pad{i}")) for i in range(50)]

    e.apply(version(0), [])
    stop = threading.Event()
    failures: list[str] = []
    counts = [0] * 8

    def reader(slot: int) -> None:
        try:
            while not stop.is_set():
                if not e.is_enabled("child", f"user-{slot}", {"plan": "pro"}):
                    failures.append("torn read")
                    return
                d = e.explain("child", "u")
                if d is None or d["reason"] != "STATIC":
                    failures.append(f"explain {d}")
                    return
                counts[slot] += 1
        except Exception as err:  # pragma: no cover - reported below
            failures.append(repr(err))

    def writer() -> None:
        k = 0
        while not stop.is_set():
            k += 1
            e.apply(version(k), [])

    threads = [threading.Thread(target=reader, args=(i,)) for i in range(8)] + [threading.Thread(target=writer)]
    for t in threads:
        t.start()
    time.sleep(1.5)
    stop.set()
    for t in threads:
        t.join()
    assert failures == []
    assert all(c > 0 for c in counts), counts


def test_evaluation_releases_the_gil() -> None:
    """A thread spinning on evaluation does not starve another Python thread.
    Smoke test only: proves no deadlock and progress on both sides."""
    e = Engine()
    e.upsert_flag_json(PAID_ONLY)
    stop = threading.Event()
    ticks = [0]

    def ticker() -> None:
        while not stop.is_set():
            ticks[0] += 1
            time.sleep(0.001)

    t = threading.Thread(target=ticker)
    t.start()
    n = 0
    end = time.monotonic() + 0.3
    while time.monotonic() < end:
        e.is_enabled("paid-only", "u", {"plan": "pro"})
        n += 1
    stop.set()
    t.join()
    assert n > 1000 and ticks[0] > 10
