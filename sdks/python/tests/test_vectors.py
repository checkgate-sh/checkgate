"""The shared evaluation vectors (sdks/conformance/vectors/evaluation.json):
every case, through the engine and through the full client, and every
malformed flag rejected."""

from __future__ import annotations

import json
from typing import Any

import pytest
from conftest import VECTORS
from fake_server import FakeServer

from checkgate import CheckgateClient
from checkgate._core import Engine

# UTF-8 explicitly: the default is the locale encoding (cp1252 on Windows),
# which garbles the non-ASCII user key of the `unicode-user-key` case.
DOC = json.loads(VECTORS.read_text(encoding="utf-8"))
CASES = DOC["cases"]
OPTIONAL = ("rule_index", "rollout_bucket", "variant_index")


def same(a: Any, b: Any) -> bool:
    """JSON equality: True is not 1, 1 is not "1"."""
    return json.dumps(a, sort_keys=True) == json.dumps(b, sort_keys=True)


def test_vector_file_shape() -> None:
    assert DOC["schema"] == 1
    assert len(CASES) >= 80 and len(DOC["malformed"]) >= 6


@pytest.fixture(scope="module")
def engine() -> Engine:
    e = Engine()
    rejected = e.apply([(f["key"], json.dumps(f)) for f in DOC["flags"]], [])
    assert rejected == []
    assert e.flag_count() == len(DOC["flags"])
    return e


@pytest.mark.parametrize("case", CASES, ids=[c["name"] for c in CASES])
def test_engine_case(engine: Engine, case: dict[str, Any]) -> None:
    expect = case["expect"]
    attrs = case["attributes"]
    variant = engine.get_variant(case["flag"], case["user"], attrs)
    explanation = engine.explain(case["flag"], case["user"], attrs)
    enabled = engine.is_enabled(case["flag"], case["user"], attrs)
    if expect is None:
        assert variant is None and explanation is None and enabled is False
        return
    assert enabled is expect["enabled"]
    assert variant is not None and variant[0] is expect["enabled"] and same(variant[1], expect["value"])
    assert explanation is not None
    assert explanation["enabled"] is expect["enabled"]
    assert same(explanation["value"], expect["value"])
    assert explanation["reason"] == expect["reason"]
    for key in OPTIONAL:
        assert explanation.get(key) == expect.get(key), key


@pytest.mark.parametrize("entry", DOC["malformed"], ids=[str(i) for i in range(len(DOC["malformed"]))])
def test_malformed_flag_is_rejected(entry: dict[str, Any]) -> None:
    e = Engine()
    assert e.upsert_flag_json(entry["json"]) is False
    assert e.flag_count() == 0
    with pytest.raises(ValueError):
        e.validate_flag_json(entry["json"])
    try:
        key = json.loads(entry["json"]).get("key")
    except (ValueError, AttributeError):
        key = None
    if isinstance(key, str):
        assert e.apply([(key, entry["json"])], []) == [key]
        assert not e.contains(key) and not e.is_enabled(key, "user-1")


def test_cases_through_the_client(make_client: Any) -> None:
    """The same cases through CheckgateClient fed by a server bootstrap, plus
    every malformed flag in the same bootstrap: never served."""
    server = FakeServer(heartbeat_ms=100)
    url = server.start()
    try:
        for f in DOC["flags"]:
            server.upsert(f)
        malformed_keys = []
        for entry in DOC["malformed"]:
            try:
                doc = json.loads(entry["json"])
            except ValueError:
                continue  # not JSON at all: the stream event itself is unreadable
            if not isinstance(doc.get("key"), str):
                continue  # names no flag: the client cannot even stage it
            malformed_keys.append(doc["key"])
            server.upsert(doc)
        client: CheckgateClient = make_client(url)
        client.connect(timeout=10)
        for key in malformed_keys:
            assert client.get_variant(key, "user-1") is None, key
            assert client.get_details(key, "user-1").error_code == "FLAG_NOT_FOUND"
        for case in CASES:
            expect = case["expect"]
            d = client.get_details(case["flag"], case["user"], case["attributes"], default="dflt")
            if expect is None:
                assert d.reason == "ERROR" and d.error_code == "FLAG_NOT_FOUND" and d.value == "dflt"
                assert client.get_value(case["flag"], case["user"], case["attributes"], "dflt") == "dflt"
                continue
            assert client.is_enabled(case["flag"], case["user"], case["attributes"]) is expect["enabled"]
            assert same(client.get_value(case["flag"], case["user"], case["attributes"]), expect["value"])
            assert d.checkgate_reason == expect["reason"]
            assert d.enabled is expect["enabled"] and same(d.value, expect["value"])
            for key in OPTIONAL:
                assert getattr(d, key) == expect.get(key), (case["name"], key)
        health = client.get_health()
        assert health["flag_count"] == len(DOC["flags"])
    finally:
        server.stop()
