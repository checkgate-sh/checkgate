---
title: Upgrading to 0.1.31
description: Upgrading a Checkgate server from 0.1.30 to 0.1.31, behaviour changes to check first, and how to roll back.
---

# Upgrading to 0.1.31

## At a glance

| | |
|---|---|
| Database migrations | **None** (`server/migrations/` is unchanged since 0.1.30) |
| Required configuration changes | None. One optional variable was added: `SSE_HEARTBEAT_INTERVAL_SECS` (default 15) |
| Rolling upgrade | Supported: 0.1.30 and 0.1.31 instances can share one PostgreSQL database and Redis, and changes written through either reach the other's SSE clients |
| Rollback to 0.1.30 | Supported, with no database step |
| SDK compatibility | Servers and SDKs of either version work together. Ordering between polls and stream replays needs both at 0.1.31 ([configuration consistency](./configuration-consistency.md)) |

Upgrade, mixed-version operation and rollback were tested on one database
with a 0.1.31 release candidate: the snapshot stayed byte-identical across the
upgrade, and 0.1.30 booted and served the writes made by 0.1.31 after the
rollback.

## Check before upgrading

**Configuration limits.** Flag and segment writes past these limits now get
`422`: more than 500 rules on a flag; the same segment referenced twice; more
than 50 distinct segments on a flag; a prerequisite on the flag itself, listed
twice, or more than 50; more than 100 variants; more than 500 rules in a
segment. Stored flags keep working and keep being served; only a later edit is
rejected until the flag is brought under the limits. To find such flags:

```sql
SELECT environment_id, key FROM flags
WHERE jsonb_array_length(COALESCE(data->'rules', '[]')) > 500
   OR jsonb_array_length(COALESCE(data->'prerequisites', '[]')) > 50
   OR jsonb_array_length(COALESCE(data->'variants', '[]')) > 100
   OR (SELECT count(*) FROM jsonb_array_elements(COALESCE(data->'rules','[]')) r WHERE r ? 'segment_key')
      <> (SELECT count(DISTINCT r->>'segment_key') FROM jsonb_array_elements(COALESCE(data->'rules','[]')) r WHERE r ? 'segment_key')
   OR EXISTS (SELECT 1 FROM jsonb_array_elements(COALESCE(data->'prerequisites','[]')) p WHERE p->>'flag_key' = flags.key);
```

**Other behaviour changes** (none needs a configuration change):

| Change | Who notices | Action |
|---|---|---|
| `GET /flags/snapshot` answers `500` if a stored flag does not decode, instead of silently leaving it out | Only after a hand-edited row | Repair the row |
| A token cannot create a token that outlives it; read-only tokens cannot revoke tokens | Automation that mints long-lived tokens from short-lived ones | Mint from a session, or set an expiry within the caller's |
| Webhooks and integrations no longer follow redirects; stored response bodies are capped at 1 KiB | Webhooks pointed at a URL that redirects | Point them at the final URL |
| The request log field `uri` is now `path` (no query string) | Log queries filtering on `uri` | Use `path` |
| SSE keep-alive is a named `event: heartbeat` instead of a comment; `connected` carries `heartbeat_ms` | Custom SSE clients that treat unknown events as errors | Ignore unknown event types |
| All-in-one container: the entrypoint stays PID 1 and stops PostgreSQL and Redis cleanly after the server | Anyone inspecting PIDs in the container | Keep the stop timeout at 30 s or more (`stop_grace_period` in compose) |

**SDK changes** to know when you update SDKs: `connect()` can now reject (on
401/403, or when `disconnect()`/`close()` is called first); after a 401/403 the
SDK stops retrying and keeps serving the flags it has; and SDKs drop a stream
that has been silent for 3 heartbeat intervals and reconnect. See the
[changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md)
for the full list.

## Rollback

1. Stop the 0.1.31 instances, or roll them back one at a time (mixed versions
   work).
2. Start 0.1.30 with the same configuration. No database step is needed.

Rollback stays this simple only while a release adds no migration: a server
refuses to boot against a database that holds a migration it does not know.

In all-in-one mode, the first start after upgrading may still run PostgreSQL
crash recovery once, left over from 0.1.30's last stop.
