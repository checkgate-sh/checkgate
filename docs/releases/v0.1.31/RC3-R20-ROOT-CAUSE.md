# R20 root cause: a delayed stream replay rolls back polled flags

| | |
|---|---|
| Finding | R20 (RELIABILITY-REVIEW.md), found by the RC2 extended fault tier |
| Severity | Medium: a temporary rollback that corrects itself, but it breaks the guarantee that a client never goes back to an older configuration |
| Affected | Node, web, React Native and Flutter SDKs: every release with poll fallback, v0.1.30 included |
| Not affected | `@checkgate/edge` and `@checkgate/ssr` (no stream: they poll or fetch once); the server's own state and reconciliation |
| Fixed in | RC3 (`fix/v0.1.31-rc3-consistency`); design in CONFIGURATION-CONSISTENCY.md |

## 1. What already existed (verified before changing anything)

The server already has an authoritative per-flag order, and the stream
already uses it:

- **`flag_revision_seq`**, one PostgreSQL sequence for the whole database
  (migration `20261018000024`). An insert takes `nextval` as the column
  default; every update takes a new value in a `BEFORE UPDATE` trigger, which
  runs after the row lock, so the writes to one flag get increasing revisions
  in the order they commit. Archiving keeps the revision because it changes
  nothing an SDK evaluates (`20261029000028`). A segment edit gives every flag
  that references the segment a new revision, in the same transaction
  (`rerevision_referencing_flags`).
- **`flag_tombstones`** records the last revision of each deleted flag
  (`20261018000027`), kept for at least a day and pruned on later deletes.
- **Stream events** carry the revision (`UPSERT … "revision"`, and `DELETE`
  carries the revision of the version it deleted). The bootstrap replay sends
  every flag with its revision and the recent tombstones as `DELETE` events,
  all read in one `REPEATABLE READ` transaction.
- **SDKs** drop a change event older than the revision they hold
  (`isStaleEvent`); the server orders versions the same way (`sync::Version`:
  a delete at `r` supersedes the flag at `r`).

The one path without revisions was **`GET /flags/snapshot`**, the poll
fallback: a bare array of flags. Every SDK therefore applied a poll by
replacing its flag set and **clearing** the revisions it held
(`this._revisions = new Map()`), and applied a stream replay on `ready` by
replacing its flag set with the replay, without comparing it with anything.
No new ordering source was needed; the snapshot lacked the existing one.

## 2. The failing sequence

Instrumented run of seed 173 against the unfixed Node SDK
(`node sdks/conformance/trace-r20.mjs 173`). Revisions as `key@rev`.

| t (ms) | Event | Client state |
|---|---|---|
| 670–827 | The stream fails (503, then a restart); after 3 failures the client also polls | |
| 827 | New stream request. **The server reads the replay now**: alpha@23, beta@24, epsilon@18, gamma@11 | |
| 835 | Poll | alpha@23 beta@24 epsilon@18 gamma@11 |
| 931 | Poll | alpha@**26** gamma@**25** |
| 1139 | Poll | beta@**27** |
| 1231 | Poll | epsilon@**29** gamma@**28** |
| 1305–1456 | The replay read at 827 arrives, delayed by the slow link (30 ms per write); staged | unchanged |
| **1459** | **`ready`: the replay replaces the flag set** | alpha@23 beta@24 epsilon@18 gamma@11: **every polled change undone** |
| 1504–1602 | The live events queued behind the replay (25…31) arrive and apply | back to alpha@26 … gamma@30 |

The sampler saw four flags go back:
`I2 alpha: went back from rev 26 to 23`, `beta 27 → 24`, `gamma 28 → 11`,
`epsilon 29 → 18`. The rollback lasted 122 ms here: as long as the live
events take to follow the replay over the same slow link. On a slower link,
longer.

Seed 184 is the same sequence with a delete in it. The Flutter client (this
run) held alpha as deleted at revision 19 from a poll; the delayed replay
still had alpha@1, and `ready` brought it back:
`I2 alpha: resurrected at rev 1 after delete rev 19`.

### Preconditions

1. Poll fallback is active (the stream failed `pollFallbackThreshold` times,
   default 3), so the client learns state outside the stream.
2. A new stream connects, and its bytes arrive more than a poll interval
   after the server read its replay (a slow, buffering or congested link, a
   proxy that holds the response, a client that is CPU-starved).
3. A flag changes between the replay read and the arrival, and a poll
   delivers the change first.

`connected` stops the polling, so only polls completed before `connected`
are involved.

## 3. The violated invariant

**A client never goes back to an older version of a flag** (conformance I2;
RC3 brief B3.1: older configuration cannot overwrite newer). In seed 184 also
**a deleted flag returns only at a newer revision** (I2, second half).

Nothing else failed in the RC2 extended tier: no partial state (I1), no
flapping at an unchanged revision (I4), no extra streams (I5), every
`connect()` settled (I6), every client converged (I7). The rollback is
temporary because the server subscribes to change events before it reads
the replay (`sse_handler`), so every change after the read follows the
replay on the same stream.

## 4. Reproduction

| Seed / scenario | Before the fix (this tree, macOS, 2026-11-19) | RC2 report |
|---|---|---|
| 173 | fails on Node (native engine), web, React Native, Flutter (Rust engine) | the same four |
| 184 | fails on Flutter; passes on the others this run (timing) | failed on web only |
| Named: *a delayed replay does not roll back newer polled flags (R20, seed 173)* | **fails on all four SDKs** | (new) |
| Named: *a delayed replay does not resurrect a flag deleted meanwhile (R20, seed 184)* | **fails on all four SDKs** | (new) |

The random seeds depend on timing. The two named scenarios do not: the fault
server reads the replay when the stream opens and holds its bytes
(`holdNextStream`), the scenario waits until that stream is open
(`waitHeld`), then changes or deletes flags that the polls deliver before
the replay arrives. They were run against the unfixed SDKs (a detached
worktree of `dev` 8f97832 with the new harness copied in) and fail on every
SDK; after the fix they pass on every SDK.

```bash
CONFORMANCE_SEEDS=173,184 node --test sdks/nodejs/test/faults.test.mjs
node --test --test-name-pattern=R20 sdks/nodejs/test/faults.test.mjs
node sdks/conformance/trace-r20.mjs 173          # the timeline above
node sdks/conformance/trace-r20.mjs 173 /path/to/older/sdks/nodejs/index.js
```

## 5. Other ordering paths checked

| Path | Finding |
|---|---|
| Live events reordered or late (two writes to one flag publish in the opposite order) | Already handled by `isStaleEvent` |
| A poll response arriving after the stream took over | Already handled: `connected` bumps a generation and aborts the poll |
| Old events of a replaced stream | Already handled: every handler ignores a stream that is not current |
| Events between the replay and `ready` | The server sends none (live events start after `ready`), but staged events were applied without a revision check; RC3 checks them, so a staged set can never hold an older version than it already has |
| A duplicate event | Applied again and **notified listeners again**. RC3 treats an equal version as a duplicate: no change, no notification. Same for a delete of a flag the client does not hold |
| Server: concurrent writes, multiple instances, restarts | Revisions come from one database sequence: no per-process counter, nothing to reconcile between instances (CONFIGURATION-CONSISTENCY.md §2) |

## 6. Why the fix is where it is

The SDK cannot decide which of a poll and a replay is newer without
revisions in the poll, and per-flag revisions alone cannot decide whether a
flag missing from a full set was deleted long ago or created after the set
was read. RC3 therefore:

1. adds the existing revisions and tombstones to the snapshot, behind
   `?v=2` so older SDKs get the same bare array as before;
2. has every SDK remember, per flag, when it learned it (a local counter, not
   a clock), and merge a full set with what it learned **after requesting
   it**: newer version wins; everything learned before is replaced, as
   before.

Details, invariants and compatibility: CONFIGURATION-CONSISTENCY.md. Test
results: RC3-SSE-FAULT-RESULTS.md.
