terraform {
  required_providers {
    checkgate = {
      source = "checkgate-sh/checkgate"
    }
  }
}

# Credentials can also come from CHECKGATE_URL / CHECKGATE_TOKEN.
provider "checkgate" {
  server_url = "https://flags.example.com"
  token      = var.checkgate_token # a read_write personal access token
}

variable "checkgate_token" {
  type      = string
  sensitive = true
}

variable "environment_id" {
  type = string
}

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

output "billing_enabled" {
  value = data.checkgate_flag.billing.is_enabled
}
