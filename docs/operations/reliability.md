---
title: Reliability and synchronization
description: How Checkgate keeps every SDK client consistent with PostgreSQL across many server instances, what it guarantees, and how that is tested.
---

# Reliability and synchronization

PostgreSQL is the single source of truth. Every flag write commits there first;
everything else — Redis, the per-instance broadcast channel, the SSE streams,
the SDKs' in-memory stores — is a delivery path for changes that are already
durable.

## How a change reaches SDKs

```
write ──► PostgreSQL commit ──► PUBLISH (Redis) ──► every instance's subscriber
                                                          │
                                         local broadcast channel (256 events)
                                                          │
                                              SSE stream per connected SDK
```

1. A write commits and receives a new **revision** from one global sequence.
   Deletes leave a **tombstone** carrying the flag's last revision (kept for a
   day).
2. The writing instance publishes the change event on the Redis channel
   (`REDIS_CHANNEL`, default `checkgate_updates`).
3. Each instance relays the event to its SSE clients for that environment.
4. SDKs apply an event only if it is newer than the revision they hold, so
   duplicated or reordered events are harmless; after a delete, only a strictly
   newer upsert can bring the flag back.
5. A (re)connecting SDK receives a consistent snapshot (one `REPEATABLE READ`
   transaction: flags, their segment expansions, tombstones), staged until the
   `ready` event, then live events. It subscribes to the broadcast before
   reading the snapshot, so nothing committed afterwards is missed.

## What can lose an event, and what recovers it

Redis Pub/Sub delivers at most once. Each loss has a recovery path:

| Loss | Recovery | Bound |
|---|---|---|
| Publish fails (Redis down or slow) | Event delivered to the writer's own clients at once; a `RESYNC` broadcast is retried until Redis accepts it, making every other instance's clients re-bootstrap | Redis back + up to 30 s backoff |
| An instance's subscription drops | On resubscribe, the instance makes all its clients re-bootstrap | reconnect (2 s retry) + resync spread |
| Subscription silently dead (connection open, nothing delivered — lost NAT/LB state) | Subscriber pings Redis every `REDIS_PING_INTERVAL_SECS`; an unanswered ping is treated as a lost connection | ping interval + timeout (≤ 5 s) |
| **Writer dies after commit, before publishing** | Reconciliation (below) — no instance holds the event, so only PostgreSQL can tell | reconcile interval + grace |
| Change committed with no event (manual SQL fix, failed segment re-expansion) | Reconciliation | reconcile interval + grace |
| SSE client > 256 events behind | Stream closed; client reconnects and re-bootstraps | SDK reconnect delay |
| SSE client stopped reading entirely | Kernel aborts the connection after `TCP_USER_TIMEOUT_SECS` (Linux), freeing it | `TCP_USER_TIMEOUT_SECS` |
| Network between SDK and server drops | SDK reconnects with backoff; after 3 failures it also polls `/flags/snapshot` | SDK backoff (3–30 s) / poll interval |

### Reconciliation

Each instance keeps an index of the newest revision it has relayed for every
flag (one small entry per flag and recent tombstone). Every
`SYNC_RECONCILE_INTERVAL_SECS` (default 30, ±20 % jitter so replicas do not
query in step) it reads the committed revisions of all flags and tombstones
from PostgreSQL in one snapshot and compares:

- a committed revision the index has not reached is a **suspect**;
- a suspect still unrelayed `SYNC_RECONCILE_GRACE_SECS` (default 10) after it
  was first noticed is a **lost change**;
- the clients of that environment — and only that environment — are told to
  re-bootstrap, and `checkgate_sync_divergences_total` is incremented with a
  `WARN` log naming the environment and up to five flag keys.

A busy flag cannot cause false alarms: a suspect is judged by the revision
noticed first, so an event that arrives late clears it even if newer writes
happened in between. Archiving a flag no longer takes a new revision (it
publishes no event, and changes nothing an SDK evaluates), so it is not
mistaken for a lost change.

The query reads `(environment_id, key, revision)` for every flag — at 10 000
flags, a sub-10 ms index scan per instance per interval. Set
`SYNC_RECONCILE_INTERVAL_SECS=0` to disable reconciliation (not recommended
with more than one instance).

### Reconnect storms

A resync does not drop every client at once: each stream closes at a random
moment within `SSE_RESYNC_SPREAD_MS` (default 5 000), and SDKs add their own
30 % jitter. Bootstraps and snapshot polls share
`SSE_BOOTSTRAP_CONCURRENCY` database slots (default half the pool), so a wave
of reconnects queues instead of starving the REST API; a request that waits
longer than `SSE_BOOTSTRAP_QUEUE_TIMEOUT_SECS` gets `503` with a jittered
`Retry-After`.

## Guarantee

**Bounded staleness once dependencies are reachable.** When PostgreSQL is
reachable from an instance, any change committed at time *t* is held by every
client connected to that instance by

> *t* + `SYNC_RECONCILE_INTERVAL_SECS` × 1.2 + `SYNC_RECONCILE_GRACE_SECS` + `SSE_RESYNC_SPREAD_MS` + one SDK reconnect

— about 51 s with the defaults — whatever happened to the change event; in
normal operation it arrives in milliseconds (see
[performance baseline](../performance/baseline.md)). Clients that cannot reach
any instance keep their last complete flag set and converge after reconnecting.

Not guaranteed:

- **Exactly-once or in-order delivery.** Events may be duplicated or arrive out
  of order; revisions make that safe.
- **Convergence while PostgreSQL is unreachable from an instance.** That
  instance cannot bootstrap clients or reconcile; it reports not ready.
- **Instant reconciliation.** A lost event is repaired by the bound above, not
  immediately.

## How it is tested

`server/tests/reliability.rs` runs real server processes against real
PostgreSQL and Redis, with simulated SDK clients that follow the shipped SDKs'
revision, staging and reconnect rules, and injects faults with per-instance TCP
proxies:

| Scenario | Test |
|---|---|
| A. Update through one instance reaches clients of all | `a_update_on_one_instance_reaches_clients_of_every_instance` |
| B. 80 concurrent writes through two instances | `b_concurrent_writes_through_both_instances_converge_to_the_database` |
| C. Update/delete races; delete then re-create | `c_racing_update_and_delete_never_resurrects_a_flag` |
| D. Redis down for the whole cluster during writes | `d_changes_committed_during_a_redis_outage_reach_every_client` |
| E. One instance's subscription down / silently dead | `e1_…`, `e2_silently_dead_subscriber_connection_is_detected` |
| F. Instance killed and restarted; writer killed after commit; commit with no event | `f1_…`, `f2_publisher_killed_between_commit_and_publish`, `f3_…` |
| G. Slow consumer; client that never reads again | `g_slow_consumer_…`, `g2_client_that_never_reads_again_is_released` |
| H. Bootstraps racing 200 writes with links cut | `h_bootstraps_racing_writes_never_settle_on_stale_state` |
| I. SDK link dropped / down / streaming blocked (poll fallback) | `i_dropped_client_connections_reconnect_or_fall_back_to_polling` |
| J. Same keys in two environments across instances | `j_identical_keys_in_two_environments_never_cross` |

`server/tests/lifecycle.rs` covers readiness, draining, graceful shutdown,
metrics, admission control and reconciliation precision (no false resyncs
from archival or segment edits; only the affected environment resyncs).

Each test asserts convergence to PostgreSQL's committed state and records the
measured recovery window (`RECOVERY` lines; with
`CHECKGATE_TEST_ARTIFACTS=<dir>` also `recovery.jsonl` and every instance's
log). The tests use reconciliation settings of 1 s interval / 2 s grace;
production defaults are 30 s / 10 s.

```bash
CHECKGATE_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres \
CHECKGATE_TEST_REDIS_URL=redis://localhost:6379 \
CHECKGATE_REQUIRE_INTEGRATION=1 \
cargo test -p server --test reliability --test lifecycle
```

The database role needs `CREATEDB`; each test creates its own database and
Redis channel, so the suites run in parallel.
