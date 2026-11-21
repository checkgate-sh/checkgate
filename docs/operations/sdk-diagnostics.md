---
title: "SDK Diagnostics — Is Every SDK Initialized, Connected and Current?"
description: "Read Checkgate SDK health locally with getHealth(), and tell server-observed connections apart from what an SDK has actually applied."
---

# SDK diagnostics

A Checkgate SDK keeps serving the last flags it has when the server is unreachable. That is
what you want for availability, and it is also why "the app is up" says nothing about whether
its flags are current. Three different things can be observed, and they answer different
questions:

| Observation | Where | Answers |
|---|---|---|
| **Local SDK state** | `getHealth()` in the process | Is *this* SDK initialized, connected, and how old are its flags? |
| **Server-observed connections** | [SDK Health](../guide/dashboard) page, `/metrics` | Which streams does a server instance hold? |
| **Configuration convergence** | compare `lastAppliedRevision` with the flag revisions in the dashboard | Has an SDK applied the latest change? |

A server accepting a connection does not prove the SDK applied anything, and a missing
connection does not mean an SDK is serving defaults (it may be polling, or serving its offline
cache). Use the local state for decisions about a process.

## Local state: `getHealth()`

The Node.js and Python SDKs expose the same fields (schema 1; web, React Native and Flutter do
not have it yet): Node.js `client.getHealth()` with camelCase
keys, Python `client.get_health()` with snake_case keys and `_ms` suffixes on timestamps.
It reads local state only, makes no network call, and never contains the SDK key, user keys or
attributes, so it is safe to return from a health endpoint.

| Field | Meaning |
|---|---|
| `state` | `idle`, `connecting`, `streaming`, `reconnecting`, `polling`, `auth_failed`, `closed` |
| `ready` | a complete flag set from the server has been applied since `connect()` |
| `source` | where the served flags came from: `none`, `offline_cache`, `server` |
| `flagCount` | flags held |
| `lastSyncAt`, `configAgeMs` | last confirmation the flags are current (bootstrap, poll, or any stream event including heartbeats), and the time since |
| `lastReceivedRevision` | highest flag revision seen in any event or snapshot, applied or not |
| `lastAppliedRevision` | highest revision in the flags held |
| `staleEventsIgnored` | events and snapshot entries rejected as not newer than what was held (late or duplicated delivery; expected occasionally) |
| `reconnectAttempts` | consecutive failed attempts since the last successful connection |
| `reconnectCount` | reconnects since the client was created |
| `polling`, `lastPollAt` | poll fallback active, and its last successful poll |
| `lastError` | `{ message, kind, at }` of the most recent sync error, `kind` one of `auth`, `network`, `http`, `parse`, `timeout`. Not cleared by a later success: an error older than `lastSyncAt` has been recovered from |
| `offlineCache` | `{ configured, hydrated, lastPersistAt }` |

### Reading it

- **Healthy:** `ready`, `state` is `streaming` (or `polling` with a recent `lastPollAt`), and
  `configAgeMs` below a few heartbeat intervals (the default heartbeat is 15 s).
- **Serving old flags:** `state` is `reconnecting` or `connecting` and `configAgeMs` keeps
  growing. Evaluations still work, from the last known flags.
- **Never reached the server:** `ready` is false. `source: "offline_cache"` means the last
  persisted snapshot is being served; `source: "none"` means every flag returns its default.
- **Misconfigured key:** `state` is `auth_failed` and `lastError.kind` is `auth`. The SDK has
  stopped retrying.

A readiness probe that fails a process with old flags:

```typescript
const h = client.getHealth()
const ok = h.ready && h.configAgeMs !== null && h.configAgeMs < 5 * 60_000
```
