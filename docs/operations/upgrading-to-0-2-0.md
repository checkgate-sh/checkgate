---
title: Upgrading to 0.2.0
description: Upgrading a Checkgate server from 0.1.31 to 0.2.0 — one additive migration, no rolling mixed-version operation, and how to roll back.
---

# Upgrading to 0.2.0

## At a glance

| | |
|---|---|
| Database migrations | **One**, additive: `20261121000029_pat_propose_scope` (allows the `propose` token scope) |
| Required configuration changes | None |
| SDK compatibility | The SDK protocol is unchanged: the `/flags/snapshot` body was byte-identical before and after the upgrade on the same data. SDKs of 0.1.31 and 0.2.0 work with either server |
| Mixed versions (rolling upgrade) | **Not supported.** Once a 0.2.0 instance has applied the migration, a 0.1.31 instance refuses to start (`migration 20261121000029 was previously applied but is missing`). Instances already running keep running — see the token rule below |
| Rollback to 0.1.31 | Needs one database step ([below](#rolling-back)) |

All of this was tested on one database with release builds of 0.1.31 and a 0.2.0 candidate
(`bench/ab/upgrade-rollback.sh`, then the rollback steps below).

## Upgrade

1. Replace every instance with 0.2.0. Plan for all 0.1.31 instances to be stopped before any of
   them restarts: after the first 0.2.0 instance starts, a restarted 0.1.31 instance exits at
   boot.
2. **Create no `propose` tokens until no 0.1.31 instance is serving.** A 0.1.31 server does not
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

3. Remove the migration's record, so the 0.1.31 migrator accepts the database:

   ```sql
   DELETE FROM _sqlx_migrations WHERE version = 20261121000029;
   ```

   The widened scope check stays in place; 0.1.31 never writes `propose`, so it is harmless.
   Upgrading again re-applies the migration.
4. Start 0.1.31.

## What else changes

See the [changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md) for the full
list: new SDK surfaces (OpenFeature provider, Python SDK, framework integrations), the
evaluation playground endpoint, SDK diagnostics endpoints and metrics, `propose` tokens and the
MCP server. None of them changes existing endpoints, configuration or SDK behaviour, except
that a Node.js client now marks itself ready before announcing `streaming` to status listeners.
