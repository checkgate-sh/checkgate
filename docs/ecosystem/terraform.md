---
title: "Terraform Provider — Manage feature flags & segments as code"
description: "Manage Checkgate feature flags and segments as code with the terraform-provider-checkgate provider for Terraform and OpenTofu."
---

# Terraform Provider

`terraform-provider-checkgate` manages Checkgate feature flags and segments as code with **Terraform or OpenTofu**. Review flag changes in a pull request, roll them out through your normal plan/apply pipeline, and keep every environment's flag configuration reproducible.

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

The `token` is a **personal access token** with `read_write` scope (an SDK key also works). Create one under **Settings → Access Tokens** in the dashboard. Both `server_url` and `token` can be supplied instead through the `CHECKGATE_URL` and `CHECKGATE_TOKEN` environment variables.

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

Polymorphic and nested fields are JSON strings (via `jsonencode`) with semantic JSON equality, so re-serialisation never produces spurious diffs.

### `checkgate_segment`

Takes `environment_id`, `key` (both force-new), `name`, `description`, and `rules` (a JSON string).

### Data source `checkgate_flag`

Look up an existing flag by `environment_id` + `key`.

## Example

```hcl
# A simple boolean flag with a 25% sticky rollout.
resource "checkgate_flag" "new_homepage" {
  environment_id     = var.environment_id
  key                = "new-homepage"
  description        = "Redesigned marketing homepage"
  is_enabled         = true
  rollout_percentage = 25
  tags               = ["web", "growth"]
}

# A reusable segment...
resource "checkgate_segment" "internal" {
  environment_id = var.environment_id
  key            = "internal-employees"
  name           = "Internal employees"
  rules = jsonencode([
    { attribute = "email", operator = "ends_with", values = ["@example.com"] }
  ])
}

# ...targeted by a string flag, which also A/B-splits everyone else 50/50.
resource "checkgate_flag" "checkout_button" {
  environment_id = var.environment_id
  key            = "checkout-button-color"
  flag_type      = "string"
  default_value  = jsonencode("blue")

  rules = jsonencode([
    { segment_key = "internal-employees", variant = "green" }
  ])

  variants = jsonencode([
    { weight = 50, value = "blue" },
    { weight = 50, value = "green" },
  ])

  depends_on = [checkgate_segment.internal]
}

# Read an existing flag managed elsewhere.
data "checkgate_flag" "billing" {
  environment_id = var.environment_id
  key            = "billing-v2"
}
```

## Import

Existing flags and segments can be brought under management with `terraform import`:

```bash
terraform import checkgate_flag.new_homepage <environment_id>/new-homepage
terraform import checkgate_segment.internal   <environment_id>/internal-employees
```

## Notes

- If an environment has **require_approval** enabled, an update is captured as a pending change request (HTTP 202) rather than applied — the provider surfaces this as an error, because an apply can't complete synchronously behind a review gate. Use a non-gated environment (or approve out of band) for IaC-managed flags.
- The provider is built on the shared `checkgate-go` API client.

## See also

- [Core Concepts](/guide/concepts) — flags, segments, targeting rules, variants, and prerequisites.
- Source: [`integrations/terraform-provider-checkgate/`](https://github.com/checkgate-sh/checkgate/tree/main/integrations/terraform-provider-checkgate)
