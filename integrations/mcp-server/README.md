# @checkgate/mcp-server

A [Model Context Protocol](https://modelcontextprotocol.io) server (stdio) that lets AI agents
read Checkgate feature flags and **propose** changes that a person approves.

**Current version: 0.2.0.** This is the package's first release; it publishes to npm together with
Checkgate 0.2.0. It needs a **Checkgate 0.2.0 server**: the `propose` token scope it relies on does
not exist on 0.1.x. What changed: [CHANGELOG](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md).

## What it does

- **Read tools** (always registered): the token's access, projects, environments (with
  `require_approval`), flags (paginated), one flag's configuration, targeting explained in plain
  language, flag dependencies, recent audit events, a flag compared across two environments,
  change requests, SDK connection counts.
- **Proposal tools** (only with `CHECKGATE_MCP_WRITE=1`): propose a flag change, propose a rollout
  percentage, withdraw the token owner's own pending change request.
- **No** create, approve, reject, delete, promote, archive or raw-HTTP tool, in any configuration.
  A new flag is served to SDKs at once, so people create flags; agents propose changes to them.

It runs next to the MCP client as a child process and speaks MCP over stdin/stdout. It opens no
network listener; it only makes HTTPS (or HTTP) requests to your Checkgate server.

## Installation

The MCP client starts the server, so there is usually nothing to install: point the client at
`npx -y @checkgate/mcp-server@0.2.0` (see below). To install it instead:

```bash
npm install -g @checkgate/mcp-server@0.2.0   # provides the checkgate-mcp command
```

From a source checkout: `cd integrations/mcp-server && npm ci`, then run
`node /path/to/checkgate/integrations/mcp-server/bin/checkgate-mcp.js`.

## Minimal example

1. Create a personal access token with scope **`propose`**: in the dashboard under
   **Settings → Access Tokens**, or with `POST /api/tokens` and `{"name": "agent", "scope": "propose", "expires_in_days": 30}`.
2. Add the server to your MCP client. Claude Code:

   ```bash
   claude mcp add --transport stdio checkgate \
     --env CHECKGATE_URL=https://flags.example.com \
     --env CHECKGATE_TOKEN=pat_xxxxxxxxxxxx \
     -- npx -y @checkgate/mcp-server@0.2.0
   ```

   Claude Desktop (`claude_desktop_config.json`), Cursor (`.cursor/mcp.json`) and other clients
   that read an `mcpServers` object:

   ```json
   {
     "mcpServers": {
       "checkgate": {
         "command": "npx",
         "args": ["-y", "@checkgate/mcp-server@0.2.0"],
         "env": {
           "CHECKGATE_URL": "https://flags.example.com",
           "CHECKGATE_TOKEN": "pat_xxxxxxxxxxxx"
         }
       }
     }
   }
   ```

   `pat_xxxxxxxxxxxx` is a placeholder; Checkgate tokens start with `pat_`. Keep the real token
   out of files you commit.
3. This configuration is read-only. To let the agent propose changes, add
   `"CHECKGATE_MCP_WRITE": "1"` to `env` (or `--env CHECKGATE_MCP_WRITE=1`).

On start the server logs one line to stderr, for example
`checkgate-mcp: ready (11 tools, read-only; propose guard on)`.

## Configuration

All configuration is through environment variables.

| Variable | Default | Meaning |
|---|---|---|
| `CHECKGATE_URL` | required | Server base URL. `http://` or `https://`, no `user:pass@`, query string or fragment. |
| `CHECKGATE_TOKEN` | required | Personal access token (scope `propose` recommended). Printable ASCII, at most 512 characters. |
| `CHECKGATE_MCP_WRITE` | `0` | `1` registers the three proposal tools. |
| `CHECKGATE_MCP_REQUIRE_PROPOSE` | `1` | `0` turns the propose guard off (not recommended). |
| `CHECKGATE_TIMEOUT_MS` | `10000` | Timeout for each API request, body included. 100 to 120000. |
| `CHECKGATE_MCP_MAX_RESPONSE_BYTES` | `2097152` | Largest API response body accepted. 1024 to 33554432. |

The two switches accept `1`/`true`/`yes` and `0`/`false`/`no`; any other value is a configuration
error.

## Tools

| Tool | Kind | What it does |
|---|---|---|
| `get_access_info` | read | The token's scope (`read_only`, `propose`, `read_write` or `unknown`), whether write tools are on, whether the propose guard is on. |
| `list_projects` | read | Projects the token can see. |
| `list_environments` | read | A project's environments, with `require_approval`. |
| `list_flags` | read | Flags in an environment, compact, paginated (up to 200 per page, default 50; filter by tag, optionally archived). |
| `get_flag` | read | One flag's full configuration. |
| `explain_flag` | read | Who gets which value, step by step in evaluation order (segments expanded). |
| `flag_dependencies` | read | Prerequisites (whether each exists and is enabled) and flags that depend on this one. Scans up to 5000 flags. |
| `flag_audit_events` | read | Recent audit events for an environment or one flag, only the fields that changed (up to 100, default 20). |
| `compare_flag_across_environments` | read | Evaluation-relevant differences separately from metadata differences between two environments. |
| `list_change_requests` | read | Change requests, newest first, optionally by status `pending`, `approved` or `rejected`. |
| `sdk_connection_summary` | read | Live SDK streaming connections per environment, counts only. |
| `propose_flag_change` | write | Propose changes to an existing flag: `is_enabled`, `rollout_percentage`, `description`, `default_value`, `disabled_value`, `rules`, `variants`, `prerequisites`, `tags`, `owner_email`. |
| `propose_rollout_change` | write | Propose a rollout percentage (0 to 100); same path as `propose_flag_change`. |
| `cancel_change_request` | write | Withdraw one of the token owner's own pending change requests. |

Every input is validated with bounded lengths and ranges, and unknown arguments are rejected,
before anything is sent. Read tools carry the MCP `readOnlyHint` annotation.

## Authentication and approval

The server authenticates with a Checkgate personal access token, sent only as
`Authorization: Bearer`. Tokens have one of three scopes: `read_only`, `propose` or `read_write`.
A token acts as the user who created it: it sees that user's projects, and project roles still
apply (a viewer's token cannot propose).

**Use a `propose` token.** For every request made with it, through this server or raw HTTP, the
Checkgate server enforces:

- a flag `PATCH` always becomes a pending change request (`202`), in every environment, whether or
  not that environment requires approval;
- `DELETE` of a change request works only on the token owner's own pending ones;
- every other write is `403`: creating flags, approve and reject included.

A different person must approve each change request; the server blocks self-approval. So proposals
from this server become pending change requests, and no tool creates, approves or applies one. A
`propose_*` result says `pending_review` and names the change request; follow it with
`list_change_requests` and `get_flag`.

**The propose guard** (`CHECKGATE_MCP_REQUIRE_PROPOSE`, on by default) covers tokens with another
scope. The server looks up the token's scope with `GET /api/tokens`, matching the token's display
prefix (its first 12 characters); if that fails or is ambiguous, the scope counts as `unknown`, not
`propose`. Unless the scope is `propose`, the guard:

- refuses to send a flag change unless the target environment requires approval;
- refuses `cancel_change_request` (a workspace admin's `read_write` token could withdraw anyone's
  request).

A refusal is an error result of kind `refused`; nothing is sent. If a change is applied directly
anyway (HTTP 200, possible only with a non-`propose` token), the result begins
with `WARNING` and reports `applied_immediately`.

## Runtime compatibility

- Node.js 20 or later (`engines: >=20`). CI tests the package on Node.js 22. Node.js 20 is past
  end of life, so prefer 22 or later.
- Transport: stdio only.
- Checkgate server 0.2.0 or later. The `propose` scope does not exist on a 0.1.31 server; do not
  create `propose` tokens while any 0.1.31 instance still serves the API.
- Dependencies: `@modelcontextprotocol/sdk` 1.32.2 and `zod` 4.6.7, pinned exactly.

## Errors and lifecycle

- A failed tool call is an MCP error result (`isError: true`) with
  `structuredContent.error.kind`: `auth` (401), `forbidden` (403), `not_found` (404), `conflict`
  (409), `invalid` (400/422), `rate_limited` (429, with `retry_after_seconds` when the server sends
  `Retry-After`), `server` (5xx), `http` (other status), `timeout`, `network`, `too_large`,
  `bad_response`, `refused` (a local guard) or `internal`. Messages are built from the status code,
  never from the response body.
- `too_large`: an API response over `CHECKGATE_MCP_MAX_RESPONSE_BYTES`, or a tool result over
  200,000 characters. Narrow the request (smaller `limit`, an `offset`, one flag).
- The process exits `0` when stdin closes, after answering requests already received (waiting at
  most 2 seconds), and on SIGTERM or SIGINT. A configuration error exits `2` with a message on
  stderr; an unexpected startup failure exits `1`.
- stdout carries only the MCP protocol. Diagnostics go to stderr, prefixed `checkgate-mcp:`.

## Security

- The token never appears in a URL, a tool result, an error or on stderr (stderr lines are
  scrubbed). A `CHECKGATE_URL` with `user:pass@` is refused at startup without repeating it.
- Redirects are refused: following one could send the token to another host.
- **Cleartext HTTP.** With a plain `http://` URL to a host other than `localhost`, `127.0.0.1` or
  `[::1]`, the server starts but warns on stderr: the token travels unencrypted. Use `https://`.
- **Untrusted content.** Flag descriptions, rule values, audit entries, names and change-request
  reasons are user-written. Results return them as data: in `structuredContent.data`, and in the
  text content only inside a `<checkgate-data>` block whose JSON escapes `<`, `>` and `&`, so
  content cannot close the block. A fixed note before the block says it is data, not
  instructions. This reduces prompt injection through flag content; it cannot prevent a model
  from following such text. With a `propose` token, a misled agent still cannot apply, approve or
  reject anything.
- With a non-`propose` token, the guard's "environment requires approval" check is a separate
  request from the change, so an admin turning approval off in between could let one change
  through. With a `propose` token the Checkgate server enforces the queueing itself.
- `sdk_connection_summary` reports the connections of the server instance that answered; behind a
  load balancer each instance reports only its own.

## Development

```bash
npm ci
npm test                        # unit + stdio lifecycle tests against a fake API
# end to end against a real server (needs an EMPTY database):
CHECKGATE_E2E_SERVER_BIN=../../target/debug/server \
CHECKGATE_E2E_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/mcp_e2e \
CHECKGATE_E2E_REDIS_URL=redis://127.0.0.1:6379 \
  node --test test/e2e.test.js
```

The e2e suite also prints the measured overhead of an MCP stdio tool call versus a direct HTTP
read of the same flag.

## Documentation

- [AI agents (MCP) guide and security model](https://checkgate-sh.github.io/checkgate/ecosystem/mcp)
- [Governance and approvals](https://checkgate-sh.github.io/checkgate/guide/governance)
- [Personal access tokens](https://checkgate-sh.github.io/checkgate/api-reference#personal-access-tokens)
- [Upgrading to 0.2.0](https://checkgate-sh.github.io/checkgate/operations/upgrading-to-0-2-0)
- [Changelog](https://github.com/checkgate-sh/checkgate/blob/main/CHANGELOG.md)

## License

Apache-2.0. Part of [Checkgate](https://github.com/checkgate-sh/checkgate).
