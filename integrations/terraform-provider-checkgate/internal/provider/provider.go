package provider

import (
	"context"
	"os"

	"github.com/hashicorp/terraform-plugin-framework/datasource"
	"github.com/hashicorp/terraform-plugin-framework/path"
	"github.com/hashicorp/terraform-plugin-framework/provider"
	"github.com/hashicorp/terraform-plugin-framework/provider/schema"
	"github.com/hashicorp/terraform-plugin-framework/resource"
	"github.com/hashicorp/terraform-plugin-framework/types"

	checkgate "github.com/checkgate-sh/checkgate/integrations/checkgate-go"
)

// Ensure the provider satisfies the framework interface.
var _ provider.Provider = &checkgateProvider{}

type checkgateProvider struct {
	version string
}

// New returns a provider factory for the given build version.
func New(version string) func() provider.Provider {
	return func() provider.Provider {
		return &checkgateProvider{version: version}
	}
}

// providerModel maps the provider `checkgate {}` configuration block.
type providerModel struct {
	ServerURL types.String `tfsdk:"server_url"`
	Token     types.String `tfsdk:"token"`
}

func (p *checkgateProvider) Metadata(_ context.Context, _ provider.MetadataRequest, resp *provider.MetadataResponse) {
	resp.TypeName = "checkgate"
	resp.Version = p.version
}

func (p *checkgateProvider) Schema(_ context.Context, _ provider.SchemaRequest, resp *provider.SchemaResponse) {
	resp.Schema = schema.Schema{
		MarkdownDescription: "Manage Checkgate feature flags and segments as code.",
		Attributes: map[string]schema.Attribute{
			"server_url": schema.StringAttribute{
				MarkdownDescription: "Base URL of the Checkgate server, e.g. `https://flags.example.com`. May also be set via the `CHECKGATE_URL` environment variable.",
				Optional:            true,
			},
			"token": schema.StringAttribute{
				MarkdownDescription: "A Checkgate personal access token with `read_write` scope (or an SDK key). May also be set via the `CHECKGATE_TOKEN` environment variable.",
				Optional:            true,
				Sensitive:           true,
			},
		},
	}
}

func (p *checkgateProvider) Configure(ctx context.Context, req provider.ConfigureRequest, resp *provider.ConfigureResponse) {
	var cfg providerModel
	resp.Diagnostics.Append(req.Config.Get(ctx, &cfg)...)
	if resp.Diagnostics.HasError() {
		return
	}

	// Explicit config wins over environment variables.
	serverURL := os.Getenv("CHECKGATE_URL")
	if !cfg.ServerURL.IsNull() {
		serverURL = cfg.ServerURL.ValueString()
	}
	token := os.Getenv("CHECKGATE_TOKEN")
	if !cfg.Token.IsNull() {
		token = cfg.Token.ValueString()
	}

	if serverURL == "" {
		resp.Diagnostics.AddAttributeError(path.Root("server_url"),
			"Missing Checkgate server URL",
			"Set `server_url` in the provider block or the CHECKGATE_URL environment variable.")
	}
	if token == "" {
		resp.Diagnostics.AddAttributeError(path.Root("token"),
			"Missing Checkgate token",
			"Set `token` in the provider block or the CHECKGATE_TOKEN environment variable.")
	}
	if resp.Diagnostics.HasError() {
		return
	}

	client, err := checkgate.NewClient(serverURL, token, checkgate.WithUserAgent("terraform-provider-checkgate/"+p.version))
	if err != nil {
		resp.Diagnostics.AddError("Unable to create Checkgate client", err.Error())
		return
	}

	// Hand the client to resources and data sources.
	resp.ResourceData = client
	resp.DataSourceData = client
}

func (p *checkgateProvider) Resources(_ context.Context) []func() resource.Resource {
	return []func() resource.Resource{
		NewFlagResource,
		NewSegmentResource,
	}
}

func (p *checkgateProvider) DataSources(_ context.Context) []func() datasource.DataSource {
	return []func() datasource.DataSource{
		NewFlagDataSource,
	}
}
