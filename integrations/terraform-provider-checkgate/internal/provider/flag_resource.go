package provider

import (
	"context"
	"fmt"
	"strings"

	"github.com/hashicorp/terraform-plugin-framework-jsontypes/jsontypes"
	"github.com/hashicorp/terraform-plugin-framework/path"
	"github.com/hashicorp/terraform-plugin-framework/resource"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/booldefault"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/planmodifier"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/stringdefault"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/stringplanmodifier"
	"github.com/hashicorp/terraform-plugin-framework/types"

	checkgate "github.com/checkgate-sh/checkgate/integrations/checkgate-go"
)

var (
	_ resource.Resource                = &flagResource{}
	_ resource.ResourceWithConfigure   = &flagResource{}
	_ resource.ResourceWithImportState = &flagResource{}
)

// NewFlagResource is the resource factory registered with the provider.
func NewFlagResource() resource.Resource { return &flagResource{} }

type flagResource struct {
	client *checkgate.Client
}

// flagModel maps the `checkgate_flag` schema. Polymorphic and nested fields
// (default_value, disabled_value, rules, variants, prerequisites) are modelled
// as JSON strings so the full evaluation schema is expressible without the
// provider having to mirror it attribute-by-attribute.
type flagModel struct {
	EnvironmentID     types.String `tfsdk:"environment_id"`
	Key               types.String `tfsdk:"key"`
	IsEnabled         types.Bool   `tfsdk:"is_enabled"`
	RolloutPercentage types.Int64  `tfsdk:"rollout_percentage"`
	Description       types.String `tfsdk:"description"`
	FlagType          types.String         `tfsdk:"flag_type"`
	DefaultValue      jsontypes.Normalized `tfsdk:"default_value"`
	DisabledValue     jsontypes.Normalized `tfsdk:"disabled_value"`
	Tags              types.List           `tfsdk:"tags"`
	OwnerEmail        types.String         `tfsdk:"owner_email"`
	Rules             jsontypes.Normalized `tfsdk:"rules"`
	Variants          jsontypes.Normalized `tfsdk:"variants"`
	Prerequisites     jsontypes.Normalized `tfsdk:"prerequisites"`
	ID                types.String         `tfsdk:"id"`
}

func (r *flagResource) Metadata(_ context.Context, req resource.MetadataRequest, resp *resource.MetadataResponse) {
	resp.TypeName = req.ProviderTypeName + "_flag"
}

func (r *flagResource) Schema(_ context.Context, _ resource.SchemaRequest, resp *resource.SchemaResponse) {
	requiresReplace := []planmodifier.String{stringplanmodifier.RequiresReplace()}
	resp.Schema = schema.Schema{
		MarkdownDescription: "A Checkgate feature flag in one environment.",
		Attributes: map[string]schema.Attribute{
			"environment_id": schema.StringAttribute{
				MarkdownDescription: "Environment ID the flag lives in. Changing it forces a new resource.",
				Required:            true,
				PlanModifiers:       requiresReplace,
			},
			"key": schema.StringAttribute{
				MarkdownDescription: "Unique flag key within the environment. Changing it forces a new resource.",
				Required:            true,
				PlanModifiers:       requiresReplace,
			},
			"is_enabled": schema.BoolAttribute{
				MarkdownDescription: "Whether the flag is on. Defaults to `true`.",
				Optional:            true,
				Computed:            true,
				Default:             booldefault.StaticBool(true),
			},
			"rollout_percentage": schema.Int64Attribute{
				MarkdownDescription: "Sticky percentage rollout (0–100). Omit for 100%.",
				Optional:            true,
			},
			"description": schema.StringAttribute{
				MarkdownDescription: "Human-readable description.",
				Optional:            true,
			},
			"flag_type": schema.StringAttribute{
				MarkdownDescription: "One of `boolean`, `string`, `integer`, `json`. Defaults to `boolean`.",
				Optional:            true,
				Computed:            true,
				Default:             stringdefault.StaticString("boolean"),
			},
			"default_value": schema.StringAttribute{
				MarkdownDescription: "JSON-encoded value returned when the flag is enabled and no rule matches, e.g. `jsonencode(true)` or `jsonencode({mode=\"dark\"})`.",
				Optional:            true,
				CustomType:          jsontypes.NormalizedType{},
			},
			"disabled_value": schema.StringAttribute{
				MarkdownDescription: "JSON-encoded value returned when the flag is off or the user is outside the rollout.",
				Optional:            true,
				CustomType:          jsontypes.NormalizedType{},
			},
			"tags": schema.ListAttribute{
				MarkdownDescription: "Dashboard-only labels.",
				Optional:            true,
				ElementType:         types.StringType,
			},
			"owner_email": schema.StringAttribute{
				MarkdownDescription: "Email of the person responsible for this flag.",
				Optional:            true,
			},
			"rules": schema.StringAttribute{
				MarkdownDescription: "JSON array of targeting rules. Use `jsonencode([...])`.",
				Optional:            true,
				CustomType:          jsontypes.NormalizedType{},
			},
			"variants": schema.StringAttribute{
				MarkdownDescription: "JSON array of weighted variants for A/B splits. Use `jsonencode([...])`.",
				Optional:            true,
				CustomType:          jsontypes.NormalizedType{},
			},
			"prerequisites": schema.StringAttribute{
				MarkdownDescription: "JSON array of prerequisite flags. Use `jsonencode([...])`.",
				Optional:            true,
				CustomType:          jsontypes.NormalizedType{},
			},
			"id": schema.StringAttribute{
				MarkdownDescription: "Synthetic ID: `<environment_id>/<key>`.",
				Computed:            true,
				PlanModifiers:       []planmodifier.String{stringplanmodifier.UseStateForUnknown()},
			},
		},
	}
}

func (r *flagResource) Configure(_ context.Context, req resource.ConfigureRequest, resp *resource.ConfigureResponse) {
	if req.ProviderData == nil {
		return
	}
	client, ok := req.ProviderData.(*checkgate.Client)
	if !ok {
		resp.Diagnostics.AddError("Unexpected provider data", fmt.Sprintf("expected *checkgate.Client, got %T", req.ProviderData))
		return
	}
	r.client = client
}

func (r *flagResource) Create(ctx context.Context, req resource.CreateRequest, resp *resource.CreateResponse) {
	var plan flagModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() {
		return
	}

	flag, diags := modelToFlag(ctx, &plan)
	resp.Diagnostics.Append(diags...)
	if resp.Diagnostics.HasError() {
		return
	}

	created, err := r.client.CreateFlag(ctx, plan.EnvironmentID.ValueString(), flag)
	if err != nil {
		resp.Diagnostics.AddError("Error creating flag", err.Error())
		return
	}

	resp.Diagnostics.Append(flagToModel(ctx, plan.EnvironmentID.ValueString(), created, &plan)...)
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *flagResource) Read(ctx context.Context, req resource.ReadRequest, resp *resource.ReadResponse) {
	var state flagModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}

	flag, err := r.client.GetFlag(ctx, state.EnvironmentID.ValueString(), state.Key.ValueString())
	if err != nil {
		if checkgate.NotFound(err) {
			resp.State.RemoveResource(ctx) // drifted away; let TF plan a recreate
			return
		}
		resp.Diagnostics.AddError("Error reading flag", err.Error())
		return
	}

	resp.Diagnostics.Append(flagToModel(ctx, state.EnvironmentID.ValueString(), flag, &state)...)
	resp.Diagnostics.Append(resp.State.Set(ctx, &state)...)
}

func (r *flagResource) Update(ctx context.Context, req resource.UpdateRequest, resp *resource.UpdateResponse) {
	var plan flagModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() {
		return
	}

	flag, diags := modelToFlag(ctx, &plan)
	resp.Diagnostics.Append(diags...)
	if resp.Diagnostics.HasError() {
		return
	}

	updated, err := r.client.UpdateFlag(ctx, plan.EnvironmentID.ValueString(), plan.Key.ValueString(), flag)
	if err != nil {
		resp.Diagnostics.AddError("Error updating flag", err.Error())
		return
	}

	resp.Diagnostics.Append(flagToModel(ctx, plan.EnvironmentID.ValueString(), updated, &plan)...)
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *flagResource) Delete(ctx context.Context, req resource.DeleteRequest, resp *resource.DeleteResponse) {
	var state flagModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if err := r.client.DeleteFlag(ctx, state.EnvironmentID.ValueString(), state.Key.ValueString()); err != nil {
		if checkgate.NotFound(err) {
			return // already gone
		}
		resp.Diagnostics.AddError("Error deleting flag", err.Error())
	}
}

// ImportState accepts "<environment_id>/<key>".
func (r *flagResource) ImportState(ctx context.Context, req resource.ImportStateRequest, resp *resource.ImportStateResponse) {
	envID, key, ok := strings.Cut(req.ID, "/")
	if !ok || envID == "" || key == "" {
		resp.Diagnostics.AddError("Invalid import ID", `expected "<environment_id>/<key>"`)
		return
	}
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("environment_id"), envID)...)
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("key"), key)...)
}
