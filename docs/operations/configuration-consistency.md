---
title: Configuration consistency
description: The order in which Checkgate SDKs apply flag changes from the SSE stream and from polling, what is guaranteed per flag, and where the guarantee stops.
---

# Configuration consistency

An SDK learns about flag changes in three ways: the stream's bootstrap replay
when it connects, live SSE events, and `GET /flags/snapshot` polls while the
stream is down. These can arrive out of order — for example, a replay that was
read before a reconnect can arrive after a poll that already delivered newer
state. This page describes how SDKs keep the result consistent.

## The guarantee

With a **0.1.31 server and a 0.1.31 SDK**, for each flag the SDK only moves
forward through the versions the server committed:

- it never serves an older version of a flag after a newer one, whether it
  learned them from a replay, a live event or a poll;
- it never brings back a deleted flag unless the server re-created it;
- it applies a complete flag set (replay or poll) as one step, and a partial
  or malformed one not at all — the last good configuration is kept;
- once faults heal, it converges to the server's committed state.

**The guarantee is per flag, not across flags.** Two different flags are not
ordered against each other: a client can briefly hold flag A's newest version
and flag B's previous one. Prerequisites therefore see each flag at a version
the server committed, not necessarily a combination that existed at one
instant. There is no atomic multi-flag update.

## How it works

Every flag version carries a **revision** from one PostgreSQL sequence
(`flag_revision_seq`). Revisions increase in commit order for each flag,
survive restarts, and are shared by every server instance on the same
database. SSE `update` and `DELETE` events carry them, and so does the
revisioned snapshot, [`GET /flags/snapshot?v=2`](/api-reference#revisioned-snapshot-v-2-since-0-1-31),
which also lists recently deleted flags as tombstones (kept for at least 24
hours).

The SDK keeps the revision of each flag it holds and when it learned it:

- **A live event** applies only if it is newer than what the SDK holds. A
  duplicate changes nothing and notifies no listener.
- **A complete set** (replay or poll) replaces everything the SDK learned
  *before* it sent that request. For anything learned *after* the request was
  sent, the newer revision wins. Listeners are called once the new set is in
  place, once per flag that changed.

Nothing compares revisions of different flags, and nothing depends on
wall-clock time.

## Compatibility

| Server | SDK | Behaviour |
|---|---|---|
| 0.1.31 | 0.1.31 | Monotonic per flag across stream, poll and reconnect |
| 0.1.31 | 0.1.30 or older | As before: the SDK polls the plain array. A delayed replay can briefly roll a polled flag back |
| 0.1.30 or older | 0.1.31 | The server ignores `?v=2` and answers the plain array; the SDK applies polls as 0.1.30 did. Live events stay ordered, but a poll and a replay are not ordered against each other |
| Mixed versions during a rolling upgrade | 0.1.31 | Each response is handled by its own format; full ordering once every instance runs 0.1.31 |

Upgrade servers first, then SDKs. Older SDKs need no change.

## Limits

- **Per flag only** (see above).
- **Needs a 0.1.31 server** for ordering between polls and replays.
- **Tombstone retention.** A replay or poll that arrives more than 24 hours
  after it was requested is out of scope.
- **Database restores.** Restoring PostgreSQL from a backup, or failing over
  to an asynchronous replica that lost committed transactions, can make the
  revision sequence issue a value again. SDKs that held a lost revision may
  ignore live events for that flag until their next reconnect. After a
  restore, restart the servers (clients reconnect and resynchronize) or move
  the sequence past its old maximum:
  `SELECT setval('flag_revision_seq', <old max> + 1000);`
- **Do not cache `/flags/snapshot`.** The endpoint sends no cache headers. An
  HTTP cache in front of it could serve a set older than one the client
  already applied.
- `@checkgate/edge` polls the plain snapshot without a stream; its refreshes
  are sequential, so each set is at least as new as the last.
  `@checkgate/ssr` reads one snapshot per render and keeps no state.
