# SDK connection reliability: v0.1.31 RC2

Scope: how each SDK notices a dead stream, recovers, handles a rejected key,
and reports how fresh its flags are. Closes RELIABILITY-REVIEW.md R4 and R6
and finds R19.

## Starting point (verified, not assumed)

| | Node | Web | React Native | Flutter |
|---|---|---|---|---|
| Transport | `eventsource` 4.1 | browser `EventSource` | `react-native-sse` 1.2.1 (required polyfill) | own `package:http` loop + SSE parser |
| Can it see the server's keep-alive? | **no**: it was an SSE comment, and `EventSource` never surfaces comments | **no** | **no** | yes (but no deadline) |
| Deadline on a silent stream | none | none | none | none |
| Deadline on a connection attempt | none | none | none | none |
| HTTP status of a failed stream | `error.code` | **not exposed** by browsers | `xhrStatus` | `statusCode` |
| 401/403 | retried forever; `connect()` pending forever | same | same | same |
| Stream errors reach the SDK | yes | yes | **no (R19)** | yes |

**R19 (high, new, fixed).** `react-native-sse`, which the React Native README
tells apps to install, dispatches errors only to `addEventListener('error')`;
the SDK set `onerror`, which it never calls. After a network error the
polyfill does not reconnect either. One network blip therefore left a React
Native app on stale flags until restart. Against the previous client the
shared fault suite failed 19 of its 21 scenarios that way (for example: client
`alpha` at rev 1, server at rev 34, 0 open streams). Two polyfill details made
the fix more than a one-line change: it re-opens a stream that was closed
while it dispatched that stream's error (its `open()` does not check for
CLOSED), and it waits 500 ms before every connection.

## Design

### Liveness: a named heartbeat and a deadline

- The server sends `event: heartbeat` / `data: {}` when a stream has been quiet
  for `SSE_HEARTBEAT_INTERVAL_SECS` (default 15, range 1–300). Any other event
  resets the timer, so a busy stream carries none. It is a named event because
  `EventSource` drops comments, and carries data because `EventSource` drops
  events without data.
- `connected` announces the interval: `{"environment_id": …, "heartbeat_ms": 15000}`.
- Every SDK keeps one deadline per stream:
  - **connect deadline** (`connectTimeoutMs`, 30 s): from opening the stream to
    `connected`. Covers a server that accepts TCP and never answers.
  - **idle deadline**: 3 × `heartbeat_ms` (45 s by default), pushed out by every
    event (JS) or every complete frame (Flutter). `streamIdleTimeoutMs` /
    `streamIdleTimeout` overrides it; `0` disables it.
  - On expiry the stream is closed and goes through the normal reconnect
    backoff (exponential, capped, 30 % jitter). Nothing else changes: the
    reconnect replay is staged until `ready`, as before.
- One timer per client, re-armed lazily (a timer that fires early re-schedules
  itself for the remaining time instead of being reset on every event).

**Why three intervals.** One missed heartbeat is normal jitter (a GC pause, a
busy proxy); three is the convention for "gone". The bound on noticing a dead
stream is therefore 45 s plus one reconnect, instead of the OS's TCP timeout
(about two hours by default).

**Why not periodic polling or client pings.** The stream already exists and the
server already sent a keep-alive; making it visible costs 27 bytes per quiet
interval per connection (measured: COST-EFFICIENCY-BENCHMARKS.md) and adds no
request. A periodic snapshot check would add a full flag set per client per
interval.

**Why bytes do not count in Flutter.** Flutter sees raw bytes, so it could
count any byte as life. The randomized suite found that this keeps a client
attached to a stream that trickles bytes without ever completing a frame
(seed 11). Flutter now counts complete frames, comment frames included, which
is also the JS SDKs' rule.

### Rejected keys are terminal

- 401 or 403 from `/stream` or `/flags/snapshot` stops every retry: no more
  stream attempts, no more polls. Other errors (5xx, 429, network) keep
  retrying with backoff.
- `connect()` rejects with `CheckgateAuthError` (Flutter: `CheckgateAuthException`)
  carrying only the status; the key never appears in the error or the log.
- Flags already loaded keep being served (last known good, as for any other
  outage). Calling `connect()` again retries, so an app can recover after the
  key is restored, or create a new client with a new key.
- Browsers do not reveal why an `EventSource` failed. After an attempt fails
  before `connected`, the web SDK asks `GET /flags/snapshot` with the same
  key: at most one check in flight and one per failed attempt, so it is
  bounded by the backoff. Only 401/403 changes anything.

### Staleness is visible

`getStatus()` (Flutter: `status`) returns `state` (`connecting`, `streaming`,
`reconnecting`, `polling`, `auth_failed`, `closed`), `lastSyncAt` and `error`.
`lastSyncAt` moves on a completed bootstrap or poll and on any event of a live
stream, heartbeats included: a quiet healthy stream is current, not stale.

The fallback policy is unchanged: every SDK keeps serving its last complete
flag set in every state. Nothing new fails open or closed.

### `connect()` always settles

| Situation | Before | Now |
|---|---|---|
| First bootstrap completes (stream or poll) | resolves | resolves |
| Key rejected | pending forever | rejects (`CheckgateAuthError`) |
| `disconnect()` / `close()` before it completes | pending forever | rejects |
| Server unreachable | keeps retrying | keeps retrying (documented; race it with a timeout) |

The returned promise has a no-op rejection handler attached, so code that
calls `connect()` without awaiting it does not get an unhandled rejection
(which ends a Node process); code that awaits still sees the error.

### Other fixes on the way

- **Every JS handler ignores events from a replaced or closed stream** (an
  instance guard), so a late event of an old stream cannot touch state.
- **Web (R13)**: `disconnect()` during WASM start-up no longer opens a stream
  afterwards. **One engine per client**: `connect()` created a new WASM engine
  each time but kept the flag cache, so after `disconnect()` + `connect()`
  without storage every flag the replay found unchanged was missing from the
  engine and evaluated `false`.
- **Flutter (R12)**: a loop generation ends the old SSE loop on `close()`, so a
  quick `close()` + `connect()` never runs two loops.
- **Flutter packaging (F1)**: see SDK-END-TO-END-VALIDATION.md.

## Compatibility

| | Server 0.1.30 | Server 0.1.31 |
|---|---|---|
| SDK 0.1.30 | as before | heartbeat event and `heartbeat_ms` ignored; behaviour as before |
| SDK 0.1.31 JS | connect deadline only (the old comment keep-alive is invisible) | full liveness |
| SDK 0.1.31 Flutter | connect deadline + idle deadline on the 15 s comment keep-alive | full liveness |

Public API changes are additive (`getStatus()`, `CheckgateAuthError`, two
options) except `connect()` now rejecting in the two cases above. Both used to
hang forever; the CHANGELOG lists them under "Upgrading from 0.1.30".

## Bounds

| Event | Noticed within | Then |
|---|---|---|
| Stream closed or reset | immediately | reconnect after 3 s (2 s Flutter, 5 s RN) × 2^n, ≤ 30 s, +≤ 30 % |
| Stream silently dead (no FIN, no bytes) | 3 × heartbeat = 45 s | same |
| Attempt hangs before `connected` | 30 s | same |
| Key revoked | next attempt (an open stream outlives revocation, N3) | stop; `auth_failed` |

Measured: in the live suite (1 s server heartbeat) every SDK replaced a stalled
stream in 3.09–3.15 s; in the fault suite (100 ms heartbeat) in 285–331 ms.
See SSE-FAULT-INJECTION.md and SDK-END-TO-END-VALIDATION.md.

## Known gap: R20

The extended fault tier found that a stream whose replay reaches the client
late can briefly roll back flags the client polled in the meantime (every SDK,
and v0.1.30 too). Liveness and auth are not involved; the cause is that poll
snapshots carry no revisions. Details and the proposed fix:
RELIABILITY-REVIEW.md R20, SSE-FAULT-INJECTION.md.

## Load and safety review of the changes

- **Retry amplification**: none new. The idle deadline replaces a stream at
  most once per 45 s per client, through the existing jittered backoff. A
  server stalled for longer than 45 s sees its clients reconnect over the
  backoff window, admitted by the existing bootstrap semaphore; the same
  happens today on a restart. The browser credential check adds at most one
  snapshot request per failed attempt.
- **Timers and tasks**: one watchdog per client, cleared on close
  (`disconnect()` / `close()` call `_closeStream()` / `_stopWatchdog()`). The
  React Native double close is one `setTimeout(0)` per replaced stream.
- **Secrets**: errors carry the status only; tests assert the key is absent.
- **Server input**: `heartbeat_ms` comes from the server, which only accepts
  1–300 s.
