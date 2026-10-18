---
title: "Checkgate Architecture — Control Plane, Evaluation Core & SDK Design"
description: "Deep dive into Checkgate's architecture: how the control plane, in-process evaluation core, and per-environment SSE stream work together to deliver flag decisions without a network round-trip."
---

# Architecture

Checkgate has three main layers: the **control plane** (server), the **evaluation core** (Rust library), and the **SDK clients**.

## Overview

<FlowDiagram />

A flag change is written once to the server, which persists it and pushes the delta to every
connected SDK over SSE in under 50 ms. From then on, each `isEnabled()` is a local, in-process
lookup — no network on the evaluation path. The detailed view:

```
┌──────────────────────────────────────────────────────────────────┐
│                        Checkgate Server                           │
│                                                                  │
│  Projects → Environments → Flags/Keys/Impressions                │
│                                                                  │
│  ┌──────────────────────────┐  ┌──────────┐  ┌───────────────┐  │
│  │  REST API                │  │ In-Memory│  │  SSE Stream   │  │
│  │  /api/projects/{id}/     │─▶│ FlagStore│─▶│  /stream      │  │
│  │    environments/{id}/    │  └──────────┘  └───────────────┘  │
│  │      flags               │       ▲          (per-env, scoped  │
│  └──────────────────────────┘       │           by SDK key)      │
│           │                         │ Redis subscriber           │
│           ▼                         │ applies updates            │
│  ┌──────────────┐  ┌──────────┐     │                            │
│  │  PostgreSQL  │─▶│  Redis   │─────┘                            │
│  │  (durable)   │  │ (pub/sub)│                                  │
│  └──────────────┘  └──────────┘                                  │
└──────────────────────────────────────────────────────────────────┘
           SSE (push on change, per-environment)
                    │
     ┌──────────────┼──────────────┬──────────────┐
     ▼              ▼              ▼              ▼
┌────────┐   ┌──────────┐   ┌─────────┐   ┌──────────────┐
│Node.js │   │ Browser  │   │ Flutter │   │ React Native │
│  SDK   │   │   WASM   │   │   FFI   │   │  C FFI/JSI   │
│(NAPI)  │   │   SDK    │   │   SDK   │   │     SDK      │
└────────┘   └──────────┘   └─────────┘   └──────────────┘
 isEnabled()  in-process, sub-microsecond
```

## Data Model

The server organizes data in a strict four-level hierarchy:

```
Workspace  (singleton — one per installation)
  └── Projects
        ├── project_members (project_id, user_id, role)
        └── Environments
              ├── sdk_keys   (environment_id FK)
              ├── flags      (environment_id FK)
              └── impressions (environment_id FK)
```

An SDK key is tied to one environment. When a client authenticates, the server uses the key to resolve both the project and the environment — no `project_id` or `environment_id` needs to be passed separately in the SDK.

## Control Plane (Server)

The server is a single Rust binary built with **Axum**. It is responsible for:

- Storing projects, environments, flags, and users durably in **PostgreSQL**
- Maintaining an in-memory `FlagStore` for fast SSE bootstrap
- Exposing a **REST API** for CRUD operations (project- and environment-scoped)
- Broadcasting flag changes to connected SDKs via **SSE** (`/stream`), scoped per environment
- Publishing change events to **Redis** pub/sub for multi-instance deployments
- Tracking flag evaluation events (**impressions**) from SDK clients

### Write Path

When a flag is created or updated:

1. The REST API handler writes the flag to **PostgreSQL**
2. It publishes an `UPSERT` event (with `env_id`) to the `checkgate_updates` **Redis** channel
3. The server's own Redis subscriber receives the event and updates the local in-memory `FlagStore`
4. Connected SSE clients whose SDK key matches the flag's `environment_id` receive the event

> The in-memory store is updated via the Redis subscriber loop, not directly by the write handler. This ensures the same code path runs for both local writes and cross-instance propagation.

### Multi-Instance Deployments

When running multiple Checkgate server instances behind a load balancer:

- Each instance subscribes to Redis pub/sub on startup
- A write to any instance propagates to all others via Redis
- All instances stay in sync without direct inter-node communication

## API Routes

Routes are organized by scope:

```
# Projects (workspace admin only for mutations)
GET     /api/projects
POST    /api/projects
PATCH   /api/projects/{project_id}
DELETE  /api/projects/{project_id}

# Project members
GET     /api/projects/{project_id}/members
POST    /api/projects/{project_id}/members
PATCH   /api/projects/{project_id}/members/{user_id}
DELETE  /api/projects/{project_id}/members/{user_id}

# Environments (project-scoped)
GET     /api/projects/{project_id}/environments
POST    /api/projects/{project_id}/environments
PATCH   /api/projects/{project_id}/environments/{env_id}
DELETE  /api/projects/{project_id}/environments/{env_id}

# SDK Keys (project-scoped, environment-targeted)
GET     /api/projects/{project_id}/keys
POST    /api/projects/{project_id}/keys          body: { name, environment_id }
DELETE  /api/projects/{project_id}/keys/{id}

# Flags (environment-scoped)
GET/POST         /api/environments/{env_id}/flags
GET/PATCH/DELETE /api/environments/{env_id}/flags/{key}
POST             /api/environments/{env_id}/flags/{key}/promote

# Impressions
GET  /api/environments/{env_id}/impressions       ?flag_key= &user_id= &value= &since_id=
GET  /api/environments/{env_id}/impressions/stats
POST /api/environments/{env_id}/impressions
```

## Evaluation Core (`checkgate-core`)

The core is a Rust library (`core/`) compiled into each SDK:

- **`FlagStore`** — thread-safe `DashMap` holding the in-memory flag cache
- **`evaluate(flag, user_context)`** — pure function, no I/O, no allocations on the hot path

### Evaluation Logic

```
isEnabled(flag_key, user_key, attributes)
    │
    ├── Flag not found → false
    ├── flag.is_enabled == false → false
    │
    ├── Targeting rules (first match wins)
    │   └── rule.attribute ∈ attributes AND operator matches → true
    │
    └── Rollout percentage
        ├── 0% → false
        ├── 100% → true
        └── MurmurHash3(flag_key + ":" + user_key) % 100 < percentage → true/false
```

Targeting rules bypass the rollout percentage. A user matching a targeting rule always gets `true`, even if the rollout is 0%.

### Hashing

Rollout uses **MurmurHash3** (x86/32-bit) for:
- **Speed** — non-cryptographic, very fast
- **Stability** — same user + flag always maps to the same bucket
- **Distribution** — uniform bucketing across the 0–99 range

## SSE Stream

The SSE stream (`GET /stream`) connects SDK clients to the server for real-time flag updates. It is **scoped per environment**.

### Environment Scoping via SDK Key

Each SDK key is stored with an `environment_id` in the database. When a client connects:

1. The server resolves the SDK key to an `environment_id`
2. It bootstraps the client with only the flags belonging to that environment (queried from PostgreSQL)
3. All subsequent live events are filtered to that `environment_id`

This means a single Checkgate installation can serve multiple projects with multiple environments, with each SDK client receiving only the flags relevant to its environment.

### Connection Lifecycle

1. SDK connects to `GET /stream` with `Authorization: Bearer sk_live_...`
2. Server looks up the key → resolves `environment_id`
3. Server sends `connected` event — SDK clears its local store
4. Server bootstraps the SDK by replaying all current flags for that environment
5. Server sends `ready` — SDK resolves its `connect()` promise
6. Server sends `keep-alive` pings every 15 seconds
7. On flag change, only clients in the matching environment receive the update
8. On reconnect, the SDK clears its local store and re-bootstraps from scratch, then receives `ready` again

### Events

| Event | Payload | Description |
|-------|---------|-------------|
| `connected` | `{"environment_id": "<uuid>" \| null}` | Connection established; SDK resets its store |
| `update` | JSON string | A flag was upserted or deleted in this environment |
| `ready` | flag count (string) | Bootstrap replay complete; SDK resolves `connect()` |
| (keep-alive) | `keep-alive-text` | Heartbeat every 15 seconds |

### Poll Fallback

`GET /flags/snapshot` (SDK-key-authed, no session-cookie support) returns the same
segment-expanded flag set as an SSE bootstrap, as a plain JSON array, for SDK clients whose
network cannot sustain a long-lived SSE connection at all (e.g. a corporate proxy). Official SDKs
poll this endpoint on a timer after a configurable number of consecutive failed `/stream`
reconnects, and stop polling as soon as `/stream` reconnects successfully — SSE retries continue
in the background the whole time. See [API Reference → Flags Snapshot](../api-reference.md#flags-snapshot-poll-fallback).

## Authentication

Two separate authentication mechanisms:

| Credential | Used by | How |
|------------|---------|-----|
| Email + password | Dashboard users | Issues an HttpOnly AES-256-GCM encrypted session cookie |
| SDK key (`sk_live_...`) | SDK clients | `Authorization: Bearer <key>` or `?sdk_key=` query param |
| Personal access token (`pat_...`) | CI/CD, Terraform, Kubernetes operator | `Authorization: Bearer <token>` |

SDK keys are stored in PostgreSQL, managed per-project from the **Project Settings → SDK Keys** tab. Each key is bound to one environment — it implicitly identifies both the project and the environment for the SSE stream and the other SDK endpoints. Because SDK keys ship inside browser and mobile apps, they are limited to those endpoints (stream, snapshot, reading flags, reporting impressions and events) for their own environment; everything else needs a session or a personal access token.

### Role Enforcement

| Route category | Required role |
|---|---|
| Read flags, impressions | Any authenticated user with project access |
| Write flags | `editor` or `admin` (project member) |
| Manage environments, SDK keys, project members | `admin` (project member) |
| Manage workspace users, create/delete projects | Workspace `admin` |

Workspace admins bypass all per-project membership checks.

## Impression Tracking

SDKs report evaluation events asynchronously:

```
POST /api/environments/{env_id}/impressions
Authorization: Bearer <sdk_key>

[{"flag_key": "checkout_v2", "user_id": "u123", "value": "true", "context": {...}}]
```

Impressions are stored in the `impressions` table (indexed by `environment_id`, `flag_key`, `evaluated_at`). The dashboard surfaces them on two tabs:

- **Analytics** — per-flag aggregates: total evaluations, true/false split, unique users
- **Stream** — live polling log (refreshes every 3s), filterable by `flag_key`, `user_id`, and `value`; context JSON expandable inline

The `GET /api/environments/{env_id}/impressions` endpoint supports `since_id` for efficient incremental polling — the stream tab uses this to fetch only new rows since the last poll.

## SDK Clients

Each SDK maintains its own local copy of the flag store:

| SDK | Binding | Binary |
|-----|---------|--------|
| Node.js | NAPI-RS | `.node` native addon |
| Browser | wasm-bindgen | `.wasm` + JS glue |
| React Native | C FFI via JSI | `.so` / `.dylib` |
| Flutter | `dart:ffi` | `.so` / `.dylib` |

The SDK key passed at initialization determines which project and environment the client receives. No project ID or environment ID needs to be specified separately in the SDK — the key encodes that association server-side.
