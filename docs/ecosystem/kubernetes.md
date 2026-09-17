---
title: "Kubernetes Operator — Declarative feature flags with a FeatureFlag CRD"
description: "Reconcile FeatureFlag custom resources into a Checkgate server with the checkgate-operator — manage flags declaratively with GitOps and kubectl."
---

# Kubernetes Operator

`checkgate-operator` is a Kubernetes operator that reconciles **`FeatureFlag`** custom resources into a Checkgate server. Manage feature flags declaratively alongside the rest of your cluster manifests — with GitOps and `kubectl` instead of a separate dashboard step.

```yaml
apiVersion: flags.checkgate.io/v1alpha1
kind: FeatureFlag
metadata:
  name: new-homepage
spec:
  server:
    url: https://flags.example.com
    tokenSecretRef: { name: checkgate-token }   # a read_write token in a Secret
  environmentId: "…"
  key: new-homepage
  enabled: true
  rolloutPercentage: 25
  defaultValue: true
```

## What it does

- **Create / update**: on any change to a `FeatureFlag`, the controller upserts the matching flag in Checkgate — creating it if missing, PATCHing it otherwise.
- **Delete**: a finalizer ensures the flag is removed from Checkgate when the CR is deleted, so there are no orphaned flags.
- **Drift correction**: each object re-reconciles periodically, so an out-of-band change in the dashboard is pulled back to the declared spec.
- **Status**: a `Ready` condition and `synced` field report the last outcome; failures set `Ready=False` with the reason and back off for retry.

## Spec reference

| Field | Description |
|---|---|
| `server.url` | Checkgate server base URL |
| `server.tokenSecretRef` | Secret + key holding a `read_write` token (key defaults to `token`) |
| `environmentId` | Environment the flag lives in |
| `key` | Flag key (immutable) |
| `enabled` | On/off (default `true`) |
| `rolloutPercentage` | Sticky rollout 0–100 |
| `description`, `flagType`, `tags` | Metadata / type (`boolean`\|`string`\|`integer`\|`json`) |
| `defaultValue` | Raw-JSON default value (`true`, `"blue"`, `{...}`) |
| `rules`, `variants` | Raw-JSON arrays for targeting / A-B splits |

## Install

```bash
# 1. Install the CRD.
kubectl apply -f config/crd/bases/flags.checkgate.io_featureflags.yaml

# 2. Create the token Secret (never commit it).
kubectl create secret generic checkgate-token --from-literal=token=cg_pat_xxx

# 3. Run the controller (in-cluster it uses its ServiceAccount; the generated
#    RBAC role is in config/rbac/role.yaml).
kubectl apply -f config/samples/featureflag.yaml
```

Build the manager image from `main.go` (a standard controller-runtime manager), or run it out-of-cluster against your kubeconfig for development:

```bash
go run .
```

## Example manifest

Reference a `read_write` token stored in a Secret, then declare one or more flags. Create the Secret out of band — never commit a real token:

```bash
kubectl create secret generic checkgate-token --from-literal=token=cg_pat_xxx
```

```yaml
apiVersion: flags.checkgate.io/v1alpha1
kind: FeatureFlag
metadata:
  name: new-homepage
  namespace: default
spec:
  server:
    url: https://flags.example.com
    tokenSecretRef:
      name: checkgate-token
      key: token
  environmentId: "00000000-0000-0000-0000-000000000000" # your environment id
  key: new-homepage
  enabled: true
  rolloutPercentage: 25
  description: Redesigned marketing homepage
  flagType: boolean
  defaultValue: true
  tags: ["web", "growth"]
---
apiVersion: flags.checkgate.io/v1alpha1
kind: FeatureFlag
metadata:
  name: checkout-button-color
  namespace: default
spec:
  server:
    url: https://flags.example.com
    tokenSecretRef:
      name: checkgate-token
  environmentId: "00000000-0000-0000-0000-000000000000"
  key: checkout-button-color
  flagType: string
  defaultValue: "blue"
  variants:
    - { weight: 50, value: "blue" }
    - { weight: 50, value: "green" }
```

## Notes

- The token must have `read_write` scope. Create one under **Settings → Access Tokens** in the dashboard, and store it in the referenced Secret rather than in the manifest.
- The operator is built on the shared `checkgate-go` API client.

## See also

- [Core Concepts](/guide/concepts) — flag types, rollouts, targeting rules, and variants.
- [Self-Hosting](/self-hosting) — running the Checkgate server the operator reconciles against.
- Source: [`integrations/kubernetes-operator/`](https://github.com/checkgate-sh/checkgate/tree/main/integrations/kubernetes-operator)
