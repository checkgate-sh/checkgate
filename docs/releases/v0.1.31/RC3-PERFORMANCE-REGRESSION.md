# Performance regression check: v0.1.31 RC3

Does the R20 fix weaken what RC2 measured? Base: `dev` 8f97832 (RC2 merged),
built in its own worktree and target directory. Candidate: the RC3 tree.
Same host, alternating runs, release builds. Everything here is measured;
nothing is projected.

Host: macOS 15 (Intel, 8 logical CPUs), PostgreSQL 16 and Redis 7 in Docker
on the same machine, 2026-11-19. Raw data: `results/rc3/`.

## Summary

| Area | Result | Material? |
|---|---|---|
| `GET /flags/snapshot` (bare array, what every published SDK and the public benchmark use) | Server CPU per request and throughput unchanged within run-to-run spread | No |
| `GET /flags/snapshot?v=2` (new, polls of 0.1.31 SDKs only) | +1–5 % server CPU per request; +1 338 bytes per response at 101 flags (+8.8 %) | No: only polling clients use it, and they poll only while streaming fails |
| SDK evaluation latency (Node) | Unchanged (±0.1×, all six cases) | No |
| SDK live change event | Unchanged (4.4 µs) | No |
| SDK complete set (replay on `ready`, poll) | +0.3–0.5 µs per flag (+5–18 %); a 1 000-flag reconnect takes 2.7 ms instead of 2.3 ms | No: once per reconnect or poll, not per evaluation |
| Reconnection recovery (fault suite) | p50 11–13 ms, max 430–670 ms (RC2: 11–14, 419–654) | No |
| Memory | Server RSS unchanged (bimodal in both builds); SDK: one small entry per flag, as before | No |
| Propagation p50/p95/p99 | Not rerun: the live-event path is unchanged on the server and in the SDKs; no dedicated Linux host | — (limitation kept) |

The README benchmark sections are unchanged: no published number is affected
(the public suite measures the bare-array snapshot, evaluation and
propagation, all unchanged).

## 1. Server snapshot

`bench/ab/snapshot-ab.sh` (now with `QUERY` / `CAND_QUERY`), 101 flags, 20 s
measured after 5 s warm-up, 3 alternating rounds per build and case. CPU per
request is the server process's CPU time over warm-up and window, divided by
the requests in that time.

### First candidate: a regression, investigated and removed

The first RC3 candidate read flags, revisions and tombstones in one statement
for both formats and always built a key → revision map, even for the bare
array. It cost CPU on the old path:

| Case | Base CPU µs/req (rounds) | Candidate CPU µs/req (rounds) |
|---|---|---|
| Array, 10 clients | 1 749 / 1 791 / 1 769 | 1 883 / 1 893 / 1 878 (+6 %) |
| Array, 100 clients | 1 780 / 1 781 / 1 798 | 1 926 / 1 927 / 1 947 (+8 %) |

No overlap between the builds in any round, so this was the change, not
noise. The fix: the bare array runs the **exact statement and code of
earlier releases**; only `?v=2` reads the revision column and the tombstones;
revisions are kept in a vector beside the flags and serialized as a map
without copying keys. Raw: `results/rc3/snapshot-ab/first-single-statement/`.

### Final candidate

| Case | Build | Throughput, req/s (rounds) | p50 / p99 ms (median) | CPU µs/req (rounds) | Bytes |
|---|---|---|---|---|---|
| Array, 10 clients | base | 648 / 679 / 552 | 14.8 / 26.3 | 1 741 / 1 730 / 1 775 | 15 271 |
| | RC3 | 671 / 669 / 589 | 14.5 / 22.9 | 1 750 / 1 736 / 1 809 | 15 271 |
| Array, 100 clients | base | 575 / 609 / 566 | 165 / 290 | 1 771 / 1 785 / 1 721 | 15 271 |
| | RC3 | 582 / 565 / 571 | 167 / 271 | 1 806 / 1 794 / 1 828 | 15 271 |
| Base array vs RC3 `?v=2`, 10 clients | base | 567 / 487 / 557 | 16.6 / 54.7 | 1 823 / 1 753 / 1 726 | 15 271 |
| | RC3 `v=2` | 584 / 458 / 373 | 19.0 / 65.2 | 1 752 / 1 777 / 1 954 | 16 609 |
| Base array vs RC3 `?v=2`, 100 clients | base | 585 / 551 / 587 | 166 / 308 | 1 816 / 1 863 / 1 836 | 15 271 |
| | RC3 `v=2` | 581 / 535 / 550 | 174 / 360 | 1 910 / 1 886 / 1 917 | 16 609 |

0 errors in every run. On the array the builds overlap round for round
(candidate median +0.5 % at 10 clients, +2 % at 100, inside the base's own
spread of 1 721–1 785). `?v=2` costs about 2–5 % CPU per request at 100
clients and 13 bytes per flag. The third `?v=2` round at 10 clients (373
req/s, 1 954 µs) ran at a host load of 39 against 12–16 for the others:
interference on the host, not the server (the other two rounds are within
the base's range).

The tombstone branch also costs PostgreSQL an anti-join over the
environment's tombstones (at most one day of deletions); in these runs that
was 0 rows. Raw: `results/rc3/snapshot-ab/final/`.

## 2. SDK configuration application

`bench/ab/node-apply-ab.cjs`: the Node SDK's own handlers with the native
engine, 15 alternating rounds, median of per-round means.

| Case | Flags | RC2 median µs | RC3 median µs | Change |
|---|---|---|---|---|
| Replay identical to what is held (the usual reconnect) | 100 | 225 | 256 | +14 % |
| | 1 000 | 2 274 | 2 692 | +18 % |
| Replay, every flag changed | 100 | 690 | 722 | +5 % |
| | 1 000 | 7 219 | 7 610 | +5 % |
| Poll, every flag changed (RC3: `?v=2` with revisions; RC2: array) | 100 | 671 | 722 | +8 % |
| | 1 000 | 7 064 | 7 554 | +7 % |
| One live change event | 100 | 4.41 | 4.43 | 0 % |
| | 1 000 | 4.42 | 4.38 | −1 % |

Two cheaper variants were measured on the way (each key visited once instead
of a spread key set; the incoming revision objects reused; the staged Map
passed through instead of copied to an array and back): together they took
the unchanged-replay overhead from +20–25 % (first measurement) to +14–18 %. What remains is the
per-flag version comparison and revision bookkeeping that the fix consists
of: about 0.3–0.4 µs per flag, once per complete set. Raw:
`results/rc3/node-apply-ab.jsonl`.

The web and React Native SDKs run the same code; Flutter the same algorithm
in Dart. Their fault-suite recovery times (§4) show no change.

## 3. SDK evaluation

`bench/ab/node-eval-ab.sh` (the public harness), 3 alternating runs:

| Case | RC2 median ns | RC3 median ns |
|---|---|---|
| boolean | 853 | 859 |
| targeting, 1 rule | 2 421 | 2 290 |
| targeting, 10 rules | 4 188 | 4 140 |
| percentage rollout | 1 087 | 1 085 |
| weighted multivariate | 2 539 | 2 334 |
| prerequisite chain of 5 | 3 502 | 3 395 |

Unchanged, as expected: RC3 touches no evaluation code (core, napi, WASM,
JSI and FFI untouched). Raw: `results/rc3/node-eval-ab/`.

## 4. Reconnection recovery

From the extended fault tier (RC3-SSE-FAULT-RESULTS.md), recovery after
healing, p50 / p95 / max:

| SDK | RC2 | RC3 |
|---|---|---|
| Node | 11 / 297 / 419 ms | 11 / 309 / 670 ms |
| Web | 12 / 308 / 624 ms | 11 / 294 / 430 ms |
| React Native | 11 / 277 / 654 ms | 11 / 313 / 438 ms |
| Flutter | 14 / 308 / 470 ms | 13 / 311 / 446 ms |

The same within noise (the maxima move both ways). Live, with a 1 s
heartbeat: reconnect after a dropped link 102–160 ms, half-open detected and
replaced 3.07–3.11 s, recovery 53–438 ms after a server SIGKILL and restart.

## 5. Propagation tail (RC2 open item)

RC2: pooled p99 at 1 000 clients 198 ms (v0.1.31) against 75 ms (v0.1.30)
from two contended runs; median run 84 against 76 ms. RC3 does not change the
path measured (a write's change event through Redis to SSE streams and into
the SDKs: no server change on that path, and the SDK's per-event cost is
unchanged, §2). No dedicated Linux host was available for this session, so
the suite was **not** rerun and the RC2 conclusion stands as it was: the
difference is attributed to host contention, and **it is not shown to be
resolved**. Follow-up unchanged: `bench/public/run.sh propagation` on a
dedicated Linux host.

## 6. Resources

- **Server memory**: end-of-run RSS is bimodal in both builds (about 10 or
  30 MiB at 10 clients, 16 or 35 MiB at 100), with no build-dependent
  pattern: allocator retention on macOS, not a difference.
- **SDK memory**: each flag already had a revision entry after a stream
  replay; RC3 adds one integer to it (`at`) and keeps entries after polls too
  (they used to be cleared). Tombstones are held for the deletions the server
  reports (at most one day's).
- **Network**: no new request, timer or background loop. A `?v=2` poll is
  about 13 bytes per flag larger. Streams are unchanged.
