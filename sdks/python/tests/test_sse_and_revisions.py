"""Pure-Python pieces: the SSE parser and the revision rules (ported from
sdks/nodejs/index.js)."""

from __future__ import annotations

import pytest

from checkgate._revisions import (
    Rev,
    event_revision,
    is_stale_event,
    loads,
    merge_full_set,
    parse_snapshot,
    supersedes,
)
from checkgate._sse import SSEError, SSEParser

STREAM = (
    b"\xef\xbb\xbfevent: connected\ndata: {\"heartbeat_ms\":100}\n\n"
    b": keep-alive comment\n\n"
    b"event: update\r\ndata: line1\r\ndata: line2\r\n\r\n"
    b"event: ready\rdata: 3\r\r"
    b"data\n\n"
    b"event: nodata\n\n"
    b"event: heartbeat\ndata:{}\n\n"
)
EXPECTED = [
    ("connected", '{"heartbeat_ms":100}'),
    ("update", "line1\nline2"),
    ("ready", "3"),
    ("message", ""),
    ("heartbeat", "{}"),
]


def test_parses_all_line_endings_bom_comments_and_multiline_data() -> None:
    assert SSEParser().feed(STREAM) == EXPECTED


@pytest.mark.parametrize("size", [1, 2, 3, 7, 64])
def test_any_fragmentation_gives_the_same_events(size: int) -> None:
    p = SSEParser()
    out = []
    for i in range(0, len(STREAM), size):
        out.extend(p.feed(STREAM[i : i + size]))
    assert out == EXPECTED


def test_multibyte_utf8_split_across_chunks() -> None:
    data = "event: update\ndata: café ☕\n\n".encode()
    p = SSEParser()
    out = [e for b in data for e in p.feed(bytes([b]))]
    assert out == [("update", "café ☕")]


def test_a_frame_cut_mid_way_is_never_dispatched() -> None:
    p = SSEParser()
    assert p.feed(b"event: update\ndata: {\"type\":\"UPS") == []
    assert p.feed(b"") == []  # the connection ended here: nothing more arrives


def test_size_limit() -> None:
    p = SSEParser(max_event_bytes=100)
    with pytest.raises(SSEError):
        p.feed(b"data: " + b"x" * 200)
    p = SSEParser(max_event_bytes=100)
    with pytest.raises(SSEError):
        for _ in range(20):
            p.feed(b"data: " + b"x" * 10 + b"\n")


def test_supersedes_and_stale() -> None:
    assert supersedes(Rev(2, False), Rev(1, False))
    assert supersedes(Rev(1, True), Rev(1, False))  # a delete supersedes what it removed
    assert not supersedes(Rev(1, False), Rev(1, True))
    assert not supersedes(Rev(1, False), Rev(1, False))
    assert is_stale_event(Rev(5, False), 5, True)  # duplicate
    assert is_stale_event(Rev(5, False), 4, True)  # older
    assert not is_stale_event(Rev(5, False), 5, False)  # delete of that version
    assert not is_stale_event(None, 1, True)
    assert not is_stale_event(Rev(5, False), None, True)  # pre-revision servers


def test_event_revision_and_strict_json() -> None:
    assert event_revision(3) == 3 and event_revision(3.0) == 3.0
    assert event_revision(True) is None and event_revision("3") is None and event_revision(None) is None
    with pytest.raises(ValueError):
        loads('{"revision": NaN}')


def test_merge_full_set_replaces_what_was_learned_before() -> None:
    flags = {"a": {"key": "a", "v": 1}, "gone": {"key": "gone"}}
    revs = {"a": Rev(1, False, at=1), "gone": Rev(2, False, at=1)}
    incoming = {"a": {"key": "a", "v": 3}, "b": {"key": "b"}}
    offered = {"a": Rev(3, False), "b": Rev(4, False)}
    out_flags, out_revs, rejected = merge_full_set(flags, revs, incoming, offered, started_at=1, at=9)
    assert out_flags == incoming and set(out_revs) == {"a", "b"} and rejected == 0
    assert out_revs["a"].at == 9


def test_merge_full_set_keeps_newer_learned_after_the_request_r20() -> None:
    # A poll at learn counter 5 delivered a@7 and deleted c@8 while a replay
    # requested at counter 2 (with a@3, c@4) was still on its way.
    flags = {"a": {"key": "a", "v": 7}}
    revs = {"a": Rev(7, False, at=5), "c": Rev(8, True, at=5)}
    incoming = {"a": {"key": "a", "v": 3}, "c": {"key": "c"}}
    offered = {"a": Rev(3, False), "c": Rev(4, False)}
    out_flags, out_revs, rejected = merge_full_set(flags, revs, incoming, offered, started_at=2, at=10)
    assert out_flags == {"a": {"key": "a", "v": 7}}  # no rollback, no resurrection
    assert out_revs["c"].deleted and rejected == 2


def test_merge_full_set_legacy_replaces_everything() -> None:
    out_flags, out_revs, _ = merge_full_set({"a": {}}, {"a": Rev(9, False, at=9)}, {"b": {"key": "b"}}, None, 0, 1)
    assert out_flags == {"b": {"key": "b"}} and out_revs == {}


def test_parse_snapshot() -> None:
    flags, revs = parse_snapshot({"v": 2, "flags": [{"key": "a"}], "revisions": {"a": 3}, "tombstones": {"b": 4, "a": 1}})
    assert flags == {"a": {"key": "a"}}
    assert revs == {"a": Rev(3, False), "b": Rev(4, True)}
    flags, revs = parse_snapshot([{"key": "a"}])
    assert revs is None and list(flags) == ["a"]
    for bad in ({"flags": [{"key": "a"}], "revisions": {}}, {"flags": "x"}, None, [{"nokey": 1}],
                {"flags": [], "tombstones": {"a": "1"}}, {"flags": [{"key": "a"}], "revisions": {"a": 1.5}}):
        with pytest.raises(ValueError):
            parse_snapshot(bad)
