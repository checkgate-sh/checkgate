# Configuration consistency: monotonic flag application (v0.1.31 RC3)

What an SDK guarantees about the order in which it applies flag
configuration, how, and where the guarantee stops. Fixes R20
(RC3-R20-ROOT-CAUSE.md).

## 1. The guarantee

For each flag, an SDK connected to a 0.1.31 server only ever moves forward
through the versions the server committed:

- it never serves an older version of a flag after a newer one, whether it
  learned them from the stream replay, a live event or a poll;
- it never brings back a flag after holding a deletion of it, unless the
  server re-created it (a newer revision);
- it applies a complete flag set (replay or poll) as one step, and a partial
  or malformed one not at all;
- after faults heal, it converges to the server's committed state (the
  bounds in SDK-CONNECTION-RELIABILITY.md and `server/src/sync.rs`).

The guarantee is **per flag**. Two different flags are not ordered against
each other: a client can briefly hold flag A's newest version and flag B's
previous one (as before). Prerequisites therefore see each flag at a version
the server committed, not necessarily a combination that existed at one
instant.

## 2. The ordering source

Revisions from **`flag_revision_seq`**, one PostgreSQL sequence. Nothing new
was introduced; RC3 exposes it on the one path that lacked it.

| Requirement | How it holds |
|---|---|
| Comparable within a defined scope | One flag (environment + key). Revisions of different flags share the sequence but are never compared by SDKs |
| Survive reconnections | They are database values; a reconnect reads the same ones |
| Restart | The sequence is persistent. After a PostgreSQL crash it may skip values (it is WAL-logged ahead), never repeat them. A server restart changes nothing |
| Several server instances | They share the database and therefore the sequence: no instance-local counter exists |
| Failed transactions | `nextval` is not rolled back, so a failed write leaves a gap. A revision is only ever published after its transaction committed (`publish_flag_change` runs after `COMMIT`; snapshot and bootstrap read committed rows) |
| Never reused for a different state | `nextval` never returns a value twice (no `CYCLE`). Archiving keeps a revision because it changes nothing an SDK sees; every other update takes a new one, including the re-revision after a segment edit |
| Order = commit order, per flag | The `BEFORE UPDATE` trigger runs after the row lock, so writes to one flag get increasing revisions in commit order |
| Deletions | A delete carries the revision of the version it removed. Order: `(revision, deleted)`, so a delete at `r` supersedes the flag at `r`, and a re-create (a later `nextval`) supersedes the delete. The same order as `sync::Version` on the server |

**Not covered**: restoring the database from a backup, or failing over to an
asynchronous replica that lost committed transactions, can make the sequence
issue a value again. SDKs that held a lost revision then drop live events
for that flag until their next full set (any reconnect: see §4, the rule
that a set replaces what was learned before it). After a restore, restart
the servers (clients reconnect) or move the sequence past the old maximum
(`SELECT setval('flag_revision_seq', <old max> + 1000)`). This was already
true for live events in v0.1.30.

## 3. Protocol

### `GET /flags/snapshot`

Unchanged without a query: a JSON array of flags, as in every release.

With `?v=2` (sent by SDKs from 0.1.31):

```json
{
  "v": 2,
  "flags": [ { "key": "checkout", "is_enabled": true, "rules": [] } ],
  "revisions": { "checkout": 4182 },
  "tombstones": { "old-banner": 4011 }
}
```

- `revisions`: the committed revision of every flag in `flags` (keyed by the
  flag's `key`).
- `tombstones`: flags deleted within the tombstone retention (at least 24 h)
  and not re-created, at the revision of the version deleted. The same
  tombstones the stream bootstrap sends as `DELETE` events.
- Flags, segments, revisions and tombstones come from **one SQL statement**,
  so they belong to one snapshot of the database; a revision is always served
  with the segment expansion it was committed with.
- `v` is the format answered. A `v` the server does not know (`v=3`) is
  answered with the newest it knows; an unparsable or older one with the
  array.
- A server before 0.1.31 ignores the parameter and answers the array. SDKs
  tell the formats apart by the shape of the body.

Cost: without `v` the server runs the exact statement and code of earlier
releases. With `?v=2` it reads one more column per flag and one more branch
(the tombstones) in the same statement, and the response grows by about 13
bytes per flag (1 338 bytes for 101 flags). Measured in
RC3-PERFORMANCE-REGRESSION.md.

### SSE (`/stream`)

Unchanged. Events already carried revisions, and the bootstrap already sent
tombstones.

## 4. SDK algorithm

Every SDK (Node, web, React Native, Flutter; the same logic in each, the
server tests' `SimSdk` mirrors it) keeps, per flag key:

- the flag (or nothing),
- `{ revision, deleted, at }`: the version it holds, and **`at`**, the value
  of a local counter when it learned that version. The counter increases by
  one for every applied change or set. It is not a clock and never leaves
  the client.

and two counter marks: `streamStartedAt`, taken before each stream request
(including retries the transport makes itself), and, for each poll,
`startedAt`, taken before the request.

**Live and staged events.** An event applies only if its version supersedes
the one held: a newer revision, or a delete at the same revision. An equal
version is a duplicate: nothing changes and no listener is called. Staged
events (between `connected` and `ready`) follow the same rule against the
staged set. Events without a revision (servers that predate revisions)
always apply, as before.

**A complete set** (the replay on `ready`, or a poll), requested at counter
value `startedAt`, with versions (flags and tombstones):

```text
for every key in (the set ∪ what the client holds):
    held    = the client's version, if any
    offered = the set's version, if any
    if held and held.at > startedAt and not offered.supersedes(held):
        keep held                       # learned after the request: maybe newer
    else:
        take offered (or nothing)       # learned before the request: older
```

then the result replaces the flag set in one step, and listeners are called
afterwards, once per flag that changed.

Why this is right:

- The server reads a set **after** it receives the request. Whatever the
  client learned before sending the request was read or published by the
  server earlier, so the set is at least as new for every flag, including
  flags it no longer contains (deleted, possibly long ago). Replacing is
  correct, and it is what every SDK did before.
- Whatever the client learned **after** sending the request (in practice:
  polls that completed while a delayed replay was on its way, R20) may be
  newer. Where both sides have the flag, the versions decide. Where the set
  has no entry for it at all, the flag was created after the set was read:
  a deletion between the two reads would have left a tombstone in the set.
  That holds while the two reads are less than the tombstone retention
  (24 h) apart; a replay or poll that takes a day to arrive is out of scope.
- Nothing is compared across flags, and nothing uses wall-clock time.

**Rejected or invalid input** leaves the state as it was: a poll body that is
neither an array nor a `v=2` object, a `v=2` flag without an integer
revision, a tombstone without one, a replay cut before `ready`, an event cut
mid-frame, an undecodable event.

## 5. The ten invariants of the RC3 brief

| # | Invariant | Mechanism | Evidence |
|---|---|---|---|
| 1 | Older configuration cannot overwrite newer | Per-flag version check on events; the merge on full sets | Named R20 scenarios fail before, pass after (4 SDKs); seeds 173 and 184; extended tier (RC3-SSE-FAULT-RESULTS.md) |
| 2 | Duplicate updates are safe | Equal version = stale | Named *duplicate events are harmless*; unit tests (no second notification) |
| 3 | Equivalent revisions do not change state | Equal version keeps the held flag object; the set diff skips unchanged flags | I4 (no flapping at an unchanged revision) in every scenario |
| 4 | Partial SSE messages are never applied | Unchanged: frames are applied only when complete; replays only on `ready` | Named *event cut mid-frame*, *replay cut mid-frame* |
| 5 | Snapshot replacement is atomic | The merged set is computed, then swapped in one synchronous step; listeners after | Unit tests (listeners see the whole set) |
| 6 | Reconnects alone do not change decisions | A replay equal to what is held changes nothing | I4; named *a quiet stream is not reconnected* |
| 7 | Polling and streaming use the same ordering | Both carry `flag_revision_seq` revisions and tombstones | Server test `revisioned_snapshot_carries_revisions_and_tombstones` (revisions equal the database's) |
| 8 | Recovery converges | A set replaces everything learned before its request | I7 in every scenario; server `sse_faults` randomized |
| 9 | Invalid updates keep the last-known-good state | Strict parsing; no partial application | §4; unit tests for malformed snapshots |
| 10 | Correct under concurrent server updates | Per-flag revisions follow commit order (row lock); post-commit publish reordering is caught by the version check | Live check *concurrent updates arrive in order and converge*; server `sse_faults` (writes through two instances) |

## 6. Compatibility

| Server | SDK | Behaviour |
|---|---|---|
| 0.1.31 | 0.1.31 | Monotonic per flag, across stream, poll and reconnect (§1) |
| 0.1.31 | 0.1.30 or older | As before: the SDK sends no `v`, gets the array. R20 remains possible for that SDK |
| 0.1.30 or older | 0.1.31 | The server ignores `?v=2`; the SDK sees an array and applies the poll as before (replace, revisions cleared). **No ordering guarantee between a poll and a replay: R20 remains possible.** Live events stay ordered (they carry revisions). The SDK does not treat the missing revisions as revision 0 |
| Mixed-version servers (rolling upgrade) | 0.1.31 | Whichever instance answers decides the format of that response; each response is handled by its own shape |

Nothing else changed on the wire. Older SDKs need no change.

## 7. Cost

No new request, no new timer, no background loop. Per complete set, the
merge is linear in the number of flags held plus offered; per event, one
comparison. Server: nothing on the old path; one column and one statement
branch for `?v=2`. Measured in RC3-PERFORMANCE-REGRESSION.md.

## 8. Limits

- Per flag, not across flags (§1).
- Monotonicity across poll and replay needs a 0.1.31 server (§6).
- The tombstone retention (24 h) bounds how far apart a delayed replay and a
  poll can be (§4).
- A database restore or a lossy failover can repeat revisions (§2).
- `@checkgate/edge` polls the snapshot without a stream and is unchanged (it
  sends no `v`). Its refreshes are single-flight, so each request is sent
  after the previous response arrived and is read later by the server: each
  set is at least as new as the last, which is the same argument as §4 with
  nothing learned in between. An HTTP cache in front of `/flags/snapshot`
  would break that argument for any client; the endpoint sends no cache
  headers and should not be cached. `@checkgate/ssr` reads one snapshot per
  render and holds no state across requests.
