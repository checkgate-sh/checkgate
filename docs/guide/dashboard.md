---
title: "Checkgate Dashboard"
description: "Explore the Checkgate dashboard: feature flags, targeting, approval reviews, environment comparison, and API credentials."
---

# Dashboard

The dashboard uses Checkgate's indigo brand color and gate logo for navigation, actions,
and enabled state. Amber highlights pending reviews, while gray marks inactive state.
These screenshots use example data for the fictional Vantage Robotics workspace, with
Juan Dela Cruz as its administrator.

## Overview

See flag counts, rollout status, and scheduled changes for the active environment.

![Checkgate dashboard overview with indigo navigation and the new logo](../../assets/screenshots/01-dashboard.png)

## Feature flags

Manage flag types, tags, rollout percentages, and enabled state per environment.

![Feature flags with indigo actions and enabled toggles](../../assets/screenshots/02-feature-flags.png)

## Flag editor

Edit targeting rules, ownership, tags, and prerequisites in the flag panel.

![Flag editor panel in the indigo Checkgate dashboard](../../assets/screenshots/03-flag-editor.png)

## Change requests

Review proposed changes before applying them in environments that require approval.
The requester cannot approve their own change.

![Pending change requests with indigo review actions](../../assets/screenshots/04-change-requests.png)

## Compare environments

Compare flag configuration across environments before promoting a change.

![Production and Staging flag comparison](../../assets/screenshots/05-environment-diff.png)

## Personal access tokens

Create scoped credentials for CI/CD, Terraform, and scripts from Settings.

![Personal access tokens and Juan Dela Cruz's account settings](../../assets/screenshots/06-settings-tokens.png)

## Environments

Keep Production, Staging, UAT, and Development configuration separate, with approval gates
where your team needs a review.

![Environment management in the indigo dashboard](../../assets/screenshots/07-environments.png)

## Collapsible sidebar

Collapse the sidebar to give tables and flag configuration more space.

![Feature flags with the collapsed sidebar and new Checkgate logo](../../assets/screenshots/08-sidebar-collapsed.png)
