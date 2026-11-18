# Upgrade and rollback: v0.1.30 → v0.1.31

## Summary

| Question | Answer | Evidence |
|---|---|---|
| Database migrations | **None.** `server/migrations/` is unchanged since v0.1.30. | `_sqlx_migrations` has 29 rows (latest `20261029000028`) before and after starting v0.1.31 |
| Rolling upgrade (old and new instances on one database and Redis) | Supported | Test 3 below: a change written through either version reached the SSE clients of the other |
| Rollback to v0.1.30 | **Supported and tested.** v0.1.30 boots on a database v0.1.31 has used and serves its writes. | Test 4 below |
| Configuration changes required | None. No variables were added, renamed or removed. | — |
| SSE wire format and SDK API | Unchanged | — |

## Test performed (2026-11-18)

Both versions were real release builds on one PostgreSQL 16 database and one
Redis 7: `v0.1.30` (tag, `8ef6452`) and the v0.1.31 candidate. The script is
in the PR description; it was run from a scratch directory, and its steps are:

1. **v0.1.30 creates the schema** and seeds 101 flags with the public
   benchmark seeder (`bench/public/sdk/seed.mjs`). Snapshot: 101 flags, hash
   `f28529ff…`.
2. **Upgrade.** v0.1.30 is stopped and v0.1.31 started on the same database.
   It is ready immediately, and its snapshot is **byte-identical** (101 flags,
   hash `f28529ff…`). No migration ran. A PATCH through v0.1.31 returns 200.
3. **Rolling.** v0.1.30 and v0.1.31 run side by side, each with an SDK-key SSE
   client. A PATCH through v0.1.31 appears on v0.1.30's stream, and a PATCH
   through v0.1.30 appears on v0.1.31's stream.
4. **Rollback.** Both are stopped and v0.1.30 is started alone. It is ready,
   serves 101 flags, and the flag last written through v0.1.31 has the value
   v0.1.31 wrote.

No ERROR-level log line appeared in any of the five server logs.

## Behaviour changes to know before upgrading

None of these need configuration changes. All are listed in `CHANGELOG.md`
under 0.1.31.

| Change | Who notices | Action |
|---|---|---|
| Flag and segment writes past the new limits return **422**: more than 500 rules; a segment referenced twice; more than 50 distinct segments per flag; a prerequisite on itself, duplicated, or more than 50; more than 100 variants; more than 500 rules per segment | Only configurations far beyond normal use. Existing stored flags keep working; they become editable again once brought under the limits. | Before upgrading, run the query below to find stored flags a later patch would reject. |
| `/flags/snapshot` returns **500** if a stored flag does not decode, instead of silently leaving it out | Only after a hand-edited or newer-version row; previously this silently disabled the flag for polling SDKs | Repair the row |
| A token cannot create a token that outlives it; read-only tokens cannot revoke tokens | Automation that mints long-lived tokens from short-lived ones | Mint from a session, or request an expiry within the caller's |
| Webhooks no longer follow redirects; stored response bodies are capped at 1 KiB | Webhooks pointed at a URL that redirects | Point them at the final URL |
| Request log field `uri` is now `path` (no query string) | Log queries filtering on `uri` | Use `path` |
| All-in-one container: the shell stays PID 1 and stops PostgreSQL and Redis after the server | Anyone inspecting PIDs in the container | Keep the stop timeout ≥ 30 s (`stop_grace_period` in compose) |

Stored flags a later patch would reject (run before upgrading):

```sql
SELECT environment_id, key FROM flags
WHERE jsonb_array_length(COALESCE(data->'rules', '[]')) > 500
   OR jsonb_array_length(COALESCE(data->'prerequisites', '[]')) > 50
   OR jsonb_array_length(COALESCE(data->'variants', '[]')) > 100
   OR (SELECT count(*) FROM jsonb_array_elements(COALESCE(data->'rules','[]')) r WHERE r ? 'segment_key')
      <> (SELECT count(DISTINCT r->>'segment_key') FROM jsonb_array_elements(COALESCE(data->'rules','[]')) r WHERE r ? 'segment_key')
   OR EXISTS (SELECT 1 FROM jsonb_array_elements(COALESCE(data->'prerequisites','[]')) p WHERE p->>'flag_key' = flags.key);
```

## Rollback procedure

1. Stop the v0.1.31 instances (or roll them back one at a time; mixed
   versions work, see test 3).
2. Start v0.1.30 with the same configuration. No database step is needed.

**Rollback stays safe only while v0.1.31 adds no migration.** The migrator has
no `ignore_missing`, so any binary refuses to boot against a database that
holds a migration it does not know. That is why the fix for R10 (scheduled
jobs retrying permanent errors), which needs a schema change, was deferred
from this release.

## Container image

- The image tag scheme, user (UID 10001), volumes, ports and environment
  variables are unchanged.
- In all-in-one mode, stopping the container now shuts PostgreSQL down
  cleanly; the first start after upgrading may still run crash recovery once,
  left over from v0.1.30's last stop.
