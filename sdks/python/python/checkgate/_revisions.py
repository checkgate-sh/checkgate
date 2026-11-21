"""Per-flag revision rules, ported from ``sdks/nodejs/index.js``.

Revisions come from one server-wide sequence and grow with every committed
write to a flag. See docs/operations/configuration-consistency.md.
"""

from __future__ import annotations

import json
import math
from dataclasses import dataclass
from typing import Any, Optional

#: Largest integer a JavaScript number holds exactly (``Number.isSafeInteger``).
_MAX_SAFE_INTEGER = 2**53 - 1


@dataclass
class Rev:
    """One flag version: its revision, whether it is a deletion, and ``at``,
    the learn counter value when the client learned it (never a clock)."""

    revision: float
    deleted: bool
    at: int = 0


def supersedes(a: Rev, b: Rev) -> bool:
    """Whether version ``a`` supersedes version ``b``.

    A delete carries the revision of the version it removed, so it supersedes
    that version, and anything newer supersedes both.
    """
    return a.revision > b.revision or (a.revision == b.revision and a.deleted and not b.deleted)


def is_stale_event(known: Optional[Rev], revision: Optional[float], is_upsert: bool) -> bool:
    """Whether a change event is not newer than what the client holds.

    A duplicate is stale too, so it changes nothing and notifies nobody.
    Events from servers that predate revisions (no number) always apply.
    """
    if known is None or revision is None:
        return False
    return not supersedes(Rev(revision, not is_upsert), known)


def merge_full_set(
    flags: dict[str, Any],
    revisions: dict[str, Rev],
    incoming: dict[str, Any],
    incoming_revisions: Optional[dict[str, Rev]],
    started_at: int,
    at: int,
) -> tuple[dict[str, Any], dict[str, Rev], int]:
    """The state after a complete flag set: a stream replay on "ready", or a
    poll snapshot.

    The server read ``incoming`` after the client sent the request, when the
    client's learn counter stood at ``started_at``. Everything the client
    learned before that is older than the set, so the set replaces it,
    absences included. What it learned afterwards (a poll that completed while
    a delayed replay was still on its way: R20) may be newer, so for those
    flags the newer version wins, and the set never brings back an older one.

    ``incoming_revisions`` is None for a poll answered by a server older than
    0.1.31, which sends no revisions: the set then replaces everything.

    Returns ``(flags, revisions, rejected)``, where ``rejected`` counts the
    set's own entries that lost to a newer held version.
    """
    if incoming_revisions is None:
        return dict(incoming), {}, 0
    out_flags: dict[str, Any] = {}
    out_revisions: dict[str, Rev] = {}
    rejected = 0

    def visit(key: str) -> None:
        nonlocal rejected
        held = revisions.get(key)
        offered = incoming_revisions.get(key)
        if held is not None and held.at > started_at and (offered is None or not supersedes(offered, held)):
            if offered is not None:
                rejected += 1
            out_revisions[key] = held
            if not held.deleted and key in flags:
                out_flags[key] = flags[key]
            return
        if offered is not None:
            # The set's own entry, owned by the client from here on.
            offered.at = at
            out_revisions[key] = offered
        if key in incoming and not (offered is not None and offered.deleted):
            out_flags[key] = incoming[key]

    # Every key of the set and of what the client holds, once each.
    for key in incoming:
        visit(key)
    for key in incoming_revisions:
        if key not in incoming:
            visit(key)
    for key in revisions:
        if key not in incoming and key not in incoming_revisions:
            visit(key)
    return out_flags, out_revisions, rejected


def _reject_constant(name: str) -> Any:
    raise ValueError(f"invalid JSON constant {name}")


def loads(text: str) -> Any:
    """``json.loads`` that, like JavaScript's ``JSON.parse``, rejects ``NaN``
    and ``Infinity``."""
    return json.loads(text, parse_constant=_reject_constant)


def event_revision(value: Any) -> Optional[float]:
    """The revision of a change event: a number, else None (``typeof === 'number'``)."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    if isinstance(value, float) and not math.isfinite(value):
        return None
    return value


def _safe_integer(value: Any) -> Optional[int]:
    """``Number.isSafeInteger``: the integer, or None."""
    if isinstance(value, bool):
        return None
    if isinstance(value, float):
        if not value.is_integer():
            return None
        value = int(value)
    if isinstance(value, int) and -_MAX_SAFE_INTEGER <= value <= _MAX_SAFE_INTEGER:
        return value
    return None


def parse_snapshot(body: Any) -> tuple[dict[str, Any], Optional[dict[str, Rev]]]:
    """Reads a ``GET /flags/snapshot`` body.

    ``{"v": 2, "flags", "revisions", "tombstones"}`` from a server that
    understands ``?v=2`` (0.1.31+), or a bare list of flags from an older one
    (revisions None). Raises ValueError on a malformed body, so a bad
    response never replaces the flags the client holds.
    """
    if isinstance(body, list):
        return _keyed(body), None
    if not isinstance(body, dict) or not isinstance(body.get("flags"), list):
        raise ValueError("Snapshot was not a flag set.")
    flags = _keyed(body["flags"])
    declared = body.get("revisions")
    revisions: dict[str, Rev] = {}
    for key in flags:
        revision = _safe_integer(declared.get(key)) if isinstance(declared, dict) else None
        if revision is None:
            raise ValueError("Snapshot flag without a revision.")
        revisions[key] = Rev(revision, False)
    tombstones = body.get("tombstones")
    if isinstance(tombstones, dict):
        for key, value in tombstones.items():
            revision = _safe_integer(value)
            if revision is None:
                raise ValueError("Snapshot tombstone without a revision.")
            if key not in revisions:
                revisions[key] = Rev(revision, True)
    return flags, revisions


def _keyed(flags: list[Any]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for flag in flags:
        key = flag.get("key") if isinstance(flag, dict) else None
        if not isinstance(key, str):
            raise ValueError("Snapshot flag without a key.")
        out[key] = flag
    return out


def canonical(flag: Any) -> str:
    """A flag version's identity, independent of key order.

    The stream serializes a flag from its stored ``jsonb`` (keys sorted), the
    snapshot from the flag type (declaration order): the same version.
    """
    return json.dumps(flag, sort_keys=True, separators=(",", ":"))
