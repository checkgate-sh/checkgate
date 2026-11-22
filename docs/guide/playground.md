---
title: "Evaluation Playground — Debug Targeting Before You Ship"
description: "See what a Checkgate SDK would answer for a given user and why: matched rule, rollout bucket, weighted variant, prerequisites and configuration revision."
---

# Evaluation playground <Since v="0.2.0" />

::: info Needs a 0.2.0 server
The playground is part of the 0.2.0 server and dashboard; the published 0.1.31 image does not
have it. The [quickstart](./quickstart) shows how to run 0.2.0 from a checkout.
:::

**Playground** in the dashboard sidebar answers one question: *what would an SDK return for
this user, and why?* Use it to check a targeting change before you rely on it, or to explain a
result a user reports.

1. Pick the project and environment in the top bar, then a flag.
2. Enter the targeting key (the user ID your application passes to the SDK) and the user's
   attributes as JSON, for example `{"plan": "pro", "country": "PH"}`. **Example from this
   flag's rules** fills in one value per attribute the flag's rules read.
3. Press **Evaluate**.

The result shows the value and its type, the reason, the rule that matched (with the segment
it came from), the user's rollout bucket, the weighted variant, each prerequisite's outcome,
the attributes the rules read but your context lacks, and the configuration revision that
was evaluated.

| Reason | Meaning |
|---|---|
| `DISABLED` | The flag is switched off. |
| `PREREQUISITE_FAILED` | A prerequisite flag is not satisfied. |
| `TARGETING_MATCH` | A targeting rule matched. |
| `ROLLOUT_EXCLUDED` | The user's bucket is outside the percentage rollout. |
| `SPLIT` | Hashing the user key decided: inside a partial rollout, or a weighted variant. |
| `DEFAULT` | The flag has rules; none matched. |
| `STATIC` | No rules, partial rollout or variants. |

## Same answer as your SDKs

The evaluation runs on the server with the same Rust engine every SDK embeds, on the same
segment-expanded configuration SDKs are served. An SDK that has applied the shown revision
returns the same result for the same targeting key and attributes. Attributes are strings
for SDKs; numbers and booleans you enter are evaluated as their string form, as the Node
OpenFeature provider and framework integrations send them.

## Safe to use on production

The playground only reads. It never changes a flag, writes no audit entry, records no
impression, and does not store or log the context you enter. Any member of the project can use
it (viewers included); members of other projects cannot, and cannot tell whether a flag exists.
It is also available to scripts as
[`POST /api/environments/{env_id}/flags/{key}/evaluate`](../api-reference#evaluate-a-flag-playground).
