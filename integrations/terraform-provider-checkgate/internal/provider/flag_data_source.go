package provider

import (
	"context"
	"fmt"

	"github.com/hashicorp/terraform-plugin-framework-jsontypes/jsontypes"
	"github.com/hashicorp/terraform-plugin-framework/datasource"
	"github.com/hashicorp/terraform-plugin-framework/datasource/schema"
	"github.com/hashicorp/terraform-plugin-framework/types"

	checkgate "github.com/checkgate-sh/checkgate/integrations/checkgate-go"
)

var (
	_ datasource.DataSource              = &flagDataSource{}
	_ datasource.DataSourceWithConfigure = &flagDataSource{}
)

// NewFlagDataSource is the data-source factory registered with the provider.
func NewFlagDataSource() datasource.DataSource { return &flagDataSource{} }

type flagDataSource struct {
	client *checkgate.Client
}

func (d *flagDataSource) Metadata(_ context.Context, req datasource.MetadataRequest, resp *datasource.MetadataResponse) {
	resp.TypeName = req.ProviderTypeName + "_flag"
}

func (d *flagDataSource) Schema(_ context.Context, _ datasource.SchemaRequest, resp *datasource.SchemaResponse) {
	resp.Schema = schema.Schema{
		MarkdownDescription: "Look up an existing Checkgate flag.",
		Attributes: map[string]schema.Attribute{
			"environment_id":     schema.StringAttribute{Required: true, MarkdownDescription: "Environment ID."},
			"key":                schema.StringAttribute{Required: true, MarkdownDescription: "Flag key."},
			"is_enabled":         schema.BoolAttribute{Computed: true},
			"rollout_percentage": schema.Int64Attribute{Computed: true},
			"description":        schema.StringAttribute{Computed: true},
			"flag_type":          schema.StringAttribute{Computed: true},
			"default_value":      schema.StringAttribute{Computed: true, CustomType: jsontypes.NormalizedType{}},
			"tags":               schema.ListAttribute{Computed: true, ElementType: types.StringType},
			"owner_email":        schema.StringAttribute{Computed: true},
			"id":                 schema.StringAttribute{Computed: true, MarkdownDescription: "`<environment_id>/<key>`."},
		},
	}
}

func (d *flagDataSource) Configure(_ context.Context, req datasource.ConfigureRequest, resp *datasource.ConfigureResponse) {
	if req.ProviderData == nil {
		return
	}
	client, ok := req.ProviderData.(*checkgate.Client)
	if !ok {
		resp.Diagnostics.AddError("Unexpected provider data", fmt.Sprintf("expected *checkgate.Client, got %T", req.ProviderData))
		return
	}
	d.client = client
}

type flagDataSourceModel struct {
	EnvironmentID     types.String         `tfsdk:"environment_id"`
	Key               types.String         `tfsdk:"key"`
	IsEnabled         types.Bool           `tfsdk:"is_enabled"`
	RolloutPercentage types.Int64          `tfsdk:"rollout_percentage"`
	Description       types.String         `tfsdk:"description"`
	FlagType          types.String         `tfsdk:"flag_type"`
	DefaultValue      jsontypes.Normalized `tfsdk:"default_value"`
	Tags              types.List           `tfsdk:"tags"`
	OwnerEmail        types.String         `tfsdk:"owner_email"`
	ID                types.String         `tfsdk:"id"`
}

func (d *flagDataSource) Read(ctx context.Context, req datasource.ReadRequest, resp *datasource.ReadResponse) {
	var cfg flagDataSourceModel
	resp.Diagnostics.Append(req.Config.Get(ctx, &cfg)...)
	if resp.Diagnostics.HasError() {
		return
	}

	flag, err := d.client.GetFlag(ctx, cfg.EnvironmentID.ValueString(), cfg.Key.ValueString())
	if err != nil {
		resp.Diagnostics.AddError("Error reading flag", err.Error())
		return
	}

	cfg.ID = types.StringValue(cfg.EnvironmentID.ValueString() + "/" + flag.Key)
	cfg.IsEnabled = types.BoolValue(flag.IsEnabled)
	cfg.FlagType = types.StringValue(flag.FlagType)
	cfg.Description = optString(flag.Description)
	cfg.OwnerEmail = optString(flag.OwnerEmail)
	cfg.DefaultValue = rawToNormalized(flag.DefaultValue)
	if flag.RolloutPercentage != nil {
		cfg.RolloutPercentage = types.Int64Value(int64(*flag.RolloutPercentage))
	} else {
		cfg.RolloutPercentage = types.Int64Null()
	}
	tags, d2 := types.ListValueFrom(ctx, types.StringType, flag.Tags)
	resp.Diagnostics.Append(d2...)
	cfg.Tags = tags

	resp.Diagnostics.Append(resp.State.Set(ctx, &cfg)...)
}
