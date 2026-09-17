# terraform-provider-checkgate

Manage [Checkgate](https://github.com/checkgate-sh/checkgate) feature flags and
segments as code with **Terraform or OpenTofu**. Review flag changes in a pull
request, roll them out through your normal plan/apply pipeline, and keep every
environment's flag configuration reproducible.

## Provider configuration

```hcl
terraform {
  required_providers {
    checkgate = {
      source = "checkgate-sh/checkgate"
    }
  }
}

provider "checkgate" {
  server_url = "https://flags.example.com" # or CHECKGATE_URL
  token      = var.checkgate_token         # or CHECKGATE_TOKEN
}
```

The `token` is a **personal access token** with `read_write` scope (an SDK key
also works). Create one under **Settings → Access Tokens** in the dashboard.

## Resources

### `checkgate_flag`

| Attribute | Type | |
|---|---|---|
| `environment_id` | string | **required**, forces new |
| `key` | string | **required**, forces new |
| `is_enabled` | bool | optional (default `true`) |
| `rollout_percentage` | number | optional (0–100) |
| `description` | string | optional |
| `flag_type` | string | optional (`boolean`\|`string`\|`integer`\|`json`, default `boolean`) |
| `default_value` / `disabled_value` | JSON string | optional — use `jsonencode(...)` |
| `tags` | list(string) | optional |
| `owner_email` | string | optional |
| `rules` / `variants` / `prerequisites` | JSON string | optional — use `jsonencode([...])` |

Polymorphic and nested fields are JSON strings (via `jsonencode`) with semantic
JSON equality, so re-serialisation never produces spurious diffs.

### `checkgate_segment`

`environment_id`, `key` (both force-new), `name`, `description`, and `rules`
(JSON string).

### Data source `checkgate_flag`

Look up an existing flag by `environment_id` + `key`.

### Import

```bash
terraform import checkgate_flag.new_homepage <environment_id>/new-homepage
terraform import checkgate_segment.internal   <environment_id>/internal-employees
```

## Notes

- If an environment has **require_approval** enabled, an update is captured as a
  pending change request (HTTP 202) rather than applied — the provider surfaces
  this as an error, because an apply can't complete synchronously behind a review
  gate. Use a non-gated environment (or approve out of band) for IaC-managed flags.
- See [`examples/main.tf`](examples/main.tf) for a full example.

## Development

```bash
go build ./...   # compile the provider
go vet ./...
# Acceptance tests (TF_ACC) require a running Checkgate server:
#   TF_ACC=1 CHECKGATE_URL=... CHECKGATE_TOKEN=... CHECKGATE_ENV=... go test ./...
```

Built on the shared [`checkgate-go`](../checkgate-go) API client.
