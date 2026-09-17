# checkgate-go

A small, dependency-free Go client for the [Checkgate](https://github.com/checkgate-sh/checkgate)
REST API. It backs the [Terraform provider](../terraform-provider-checkgate) and
the [Kubernetes operator](../kubernetes-operator), and is usable on its own for any
automation that manages Checkgate flags and segments.

```go
import checkgate "github.com/checkgate-sh/checkgate/integrations/checkgate-go"

c, _ := checkgate.NewClient("https://flags.example.com", token) // read_write PAT or SDK key

flag, err := c.CreateFlag(ctx, envID, &checkgate.Flag{
    Key:          "new-homepage",
    IsEnabled:    true,
    FlagType:     "boolean",
    DefaultValue: json.RawMessage("true"),
})

if _, err := c.UpdateFlag(ctx, envID, "new-homepage", flag); err == checkgate.ErrApprovalRequired {
    // the environment requires review — the change was queued, not applied
}
```

## Surface

- **Flags**: `GetFlag`, `CreateFlag`, `UpdateFlag`, `DeleteFlag`
- **Segments**: `GetSegment`, `CreateSegment`, `UpdateSegment`, `DeleteSegment`
- **Errors**: `*APIError` (carries the HTTP status), `NotFound(err)`, `ErrApprovalRequired`
- **Options**: `WithHTTPClient`, `WithUserAgent`

Auth is a Bearer token (a `read_write` personal access token, or an SDK key).
Polymorphic flag values (`default_value`, variant values, …) are `json.RawMessage`
so any of bool/string/int/JSON round-trips losslessly.

```bash
go test ./...   # httptest-based, no server required
```
