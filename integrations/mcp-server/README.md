# @checkgate/mcp-server

A [Model Context Protocol](https://modelcontextprotocol.io) server (stdio) that lets AI agents
read Checkgate feature flags and **propose** changes that a person approves.

- **Read tools** (always on): projects, environments (with `require_approval`), flags
  (paginated), one flag's configuration, targeting explained in plain language, flag dependencies,
  recent audit events, a flag compared across two environments, change requests, SDK connection
  counts per environment.
- **Proposal tools** (only with `CHECKGATE_MCP_WRITE=1`): `propose_flag_change`,
  `propose_rollout_change`, `cancel_change_request` (own pending requests).
- **No** create, approve, reject, delete, promote, archive or raw-HTTP tool, in any
  configuration. (A new flag is served to SDKs at once, so people create flags.)

Full guide and security model: [docs/ecosystem/mcp.md](../../docs/ecosystem/mcp.md).

## Use a `propose` token

Create the agent's personal access token with scope **`propose`** (Settings → Access Tokens, or
`POST /api/tokens` with `"scope": "propose"`). The Checkgate server then enforces, for every request
with that token — through this server or raw HTTP:

- a flag `PATCH` always becomes a pending change request (`202`), in every environment;
- `DELETE` of a change request works only on the owner's own pending ones;
- everything else that writes is `403`, creating flags, approve and reject included.

A different person must approve each change request (self-approval is blocked server-side).

With any other token, the **propose guard** (`CHECKGATE_MCP_REQUIRE_PROPOSE`, default on) refuses to
send a change unless the target environment requires approval. If a change is ever applied
directly (HTTP 200), the tool result begins with a loud `WARNING`.

## Configuration

| Variable | Default | |
|---|---|---|
| `CHECKGATE_URL` | required | Server base URL (`http(s)://`, no `user:pass@`, query or fragment). |
| `CHECKGATE_TOKEN` | required | Personal access token (`propose` recommended). |
| `CHECKGATE_MCP_WRITE` | `0` | `1` registers the proposal tools. |
| `CHECKGATE_MCP_REQUIRE_PROPOSE` | `1` | `0` turns the propose guard off. |
| `CHECKGATE_TIMEOUT_MS` | `10000` | Whole-request timeout, 100–120000. |
| `CHECKGATE_MCP_MAX_RESPONSE_BYTES` | `2097152` | Largest API response accepted. |

Claude Code:

```bash
claude mcp add --transport stdio checkgate \
  --env CHECKGATE_URL=https://flags.example.com \
  --env CHECKGATE_TOKEN=pat_your_propose_token \
  --env CHECKGATE_MCP_WRITE=1 \
  -- node /path/to/checkgate/integrations/mcp-server/bin/checkgate-mcp.js
```

Claude Desktop (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "checkgate": {
      "command": "node",
      "args": ["/path/to/checkgate/integrations/mcp-server/bin/checkgate-mcp.js"],
      "env": {
        "CHECKGATE_URL": "https://flags.example.com",
        "CHECKGATE_TOKEN": "pat_your_propose_token",
        "CHECKGATE_MCP_WRITE": "1"
      }
    }
  }
}
```

## Untrusted content

Flag descriptions, rule values, audit entries, names and change-request reasons are user-written.
Results return them as data: in `structuredContent.data`, and in the text content only inside a
`<checkgate-data>` block whose JSON escapes `<`, `>` and `&`, so content cannot close the block. A
fixed note before the block says it is data, not instructions.

## Behaviour details

- The token is sent only as `Authorization: Bearer`; it never appears in a URL, a result, an error
  or on stderr. Redirects are refused (they could carry the token elsewhere).
- Error results carry a stable `kind`: `auth` (401), `forbidden` (403), `not_found` (404),
  `conflict` (409), `invalid` (400/422), `rate_limited` (429, with `retry_after_seconds`), `server`
  (5xx), `http`, `timeout`, `network`, `too_large`, `bad_response`, `refused` (a local guard), and
  `internal`. Messages are built from the status code only, never from the response body.
- The process exits 0 when stdin closes (after answering requests already received, at most 2 s)
  and on SIGTERM/SIGINT. A configuration error exits 2.
- The token's scope is read from `GET /api/tokens` by matching this token's display prefix; if
  that is ambiguous or fails, the scope counts as unknown (not `propose`).

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
