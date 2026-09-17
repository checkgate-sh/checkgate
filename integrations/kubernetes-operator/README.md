# checkgate-operator

A Kubernetes operator that reconciles **`FeatureFlag`** custom resources into a
[Checkgate](https://github.com/checkgate-sh/checkgate) server — manage feature
flags declaratively alongside the rest of your cluster manifests, with GitOps and
`kubectl` instead of a separate dashboard step.

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

- **Create / update**: on any change to a `FeatureFlag`, the controller upserts the
  matching flag in Checkgate (create if missing, PATCH otherwise).
- **Delete**: a finalizer ensures the flag is removed from Checkgate when the CR is
  deleted — no orphaned flags.
- **Drift correction**: each object re-reconciles periodically, so an out-of-band
  change in the dashboard is pulled back to the declared spec.
- **Status**: a `Ready` condition and `synced` field report the last outcome;
  failures set `Ready=False` with the reason and back off for retry.

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

Build the manager image from `main.go` (a standard controller-runtime manager),
or run it out-of-cluster against your kubeconfig for development:

```bash
go run .
```

## Development

```bash
go build ./...
go test ./...        # reconcile logic is unit-tested with a fake client + fake Checkgate server

# Regenerate deepcopy + CRD/RBAC after changing api/v1alpha1 types:
go run sigs.k8s.io/controller-tools/cmd/controller-gen@v0.16.5 object paths=./api/...
go run sigs.k8s.io/controller-tools/cmd/controller-gen@v0.16.5 \
  rbac:roleName=checkgate-operator-role crd paths=./... \
  output:crd:artifacts:config=config/crd/bases output:rbac:artifacts:config=config/rbac
```

Built on the shared [`checkgate-go`](../checkgate-go) API client.
