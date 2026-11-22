---
title: Upgrading to 0.2.0
description: Upgrading a Checkgate server from 0.1.31 to 0.2.0 — two additive migrations, no rolling mixed-version operation, and how to roll back.
---

# Upgrading to 0.2.0

## At a glance

| | |
|---|---|
| Database migrations | **Two**, additive: `20261121000029_pat_propose_scope` (allows the `propose` token scope) and `20261122000030_change_request_create` (adds `change_requests.action`, default `update`, and a unique index: one pending flag creation per key) |
| Required configuration changes | None |
| SDK compatibility | The SDK protocol is unchanged: the `/flags/snapshot` body was byte-identical before and after the upgrade on the same data. SDKs of 0.1.31 and 0.2.0 work with either server |
| Behaviour change | In an environment that requires approval, creating a flag that is not inert (enabled, or with its own `disabled_value`) answers `202 Accepted` with a pending change request instead of `200` — see [Governance](/guide/governance#change-requests-approvals) |
| Mixed versions (rolling upgrade) | **Not supported.** Once a 0.2.0 instance has applied the migrations, a 0.1.31 instance refuses to start (`migration 20261121000029 was previously applied but is missing`). Instances already running keep running — see the token rule below |
| Rollback to 0.1.31 | Needs one database step ([below](#rolling-back)) |

All of this was tested with release builds of 0.1.31 and a 0.2.0 candidate on real databases
(`bench/ab/upgrade_rollback_020.py`: upgrade, mixed versions, the rollback below, re-upgrade,
restore from a backup, and an upgrade killed half-way).

## Upgrade

1. **Back up the database** (for example `pg_dump --format=custom --file=checkgate-0.1.31.dump
   "$DATABASE_URL"`). It is the only way back that also undoes data written by 0.2.0.
2. Replace every instance with 0.2.0. Plan for all 0.1.31 instances to be stopped before any of
   them restarts: after the first 0.2.0 instance starts, a restarted 0.1.31 instance exits at
   boot.
3. **Create no `propose` tokens until no 0.1.31 instance is serving.** A 0.1.31 server does not
   know the scope: it refuses a `propose` token's flag writes (anything that is not
   `read_write` is read-only there) but **lets it create a `read_write` token**, because 0.1.31
   only caps tokens created by `read_only` tokens. A request routed to an old instance during the
   upgrade could therefore turn an agent's token into a full one.

## Rolling back

To run 0.1.31 again on the same database:

1. Stop every 0.2.0 instance.
2. Revoke all `propose` tokens — 0.1.31 would let them mint `read_write` tokens:

   ```sql
   DELETE FROM personal_access_tokens WHERE scope = 'propose';
   ```

3. Cancel pending flag creations. 0.1.31 does not know the `create` action: approving one there
   answers `404` while the key does not exist, but would apply it as a patch once someone has
   created that key:

   ```sql
   UPDATE change_requests SET status = 'cancelled'
    WHERE action = 'create' AND status = 'pending';
   ```

4. Remove the migrations' records, so the 0.1.31 migrator accepts the database:

   ```sql
   DELETE FROM _sqlx_migrations WHERE version IN (20261121000029, 20261122000030);
   ```

   The widened scope check, the `action` column and its index stay in place; 0.1.31 never writes
   `propose` and its inserts get `action = 'update'`, so they are harmless. Upgrading again
   re-applies both migrations (they are idempotent).
5. Start 0.1.31.

### Rolling back by restoring the backup

The SQL steps above keep everything 0.2.0 wrote (flags, approvals, audit entries). To return to
exactly the pre-upgrade state instead — losing every change made since — stop all instances and
restore the backup from upgrade step 1 into an empty database:

```bash
pg_restore --no-owner --dbname "$EMPTY_DATABASE_URL" checkgate-0.1.31.dump
```

Point 0.1.31 at that database. SDKs reconnect and receive the restored flags. Changes made under
0.2.0 are gone; there is no merge.

## If the upgrade is interrupted

Each migration runs in its own transaction, so a 0.2.0 process killed during the upgrade leaves
every migration either fully applied or not at all — never a half-applied one (tested by killing
0.2.0 while migration 30 waited on a lock: 29 was applied, 30 was not, and the schema had no
partial change). Starting 0.2.0 again applies whatever is missing and serves the same data. A
0.1.31 instance refuses such a database as soon as one 0.2.0 migration is recorded; either finish
the upgrade by starting 0.2.0, or roll back with the steps above (only the recorded rows need
deleting).

## What else changes

See the [changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md) for the full
list: new SDK surfaces (OpenFeature provider, Python SDK, framework integrations), the
evaluation playground endpoint, SDK diagnostics endpoints and metrics, `propose` tokens and the
MCP server. None of them changes existing endpoints, configuration or SDK behaviour, except
that a Node.js client now marks itself ready before announcing `streaming` to status listeners.
