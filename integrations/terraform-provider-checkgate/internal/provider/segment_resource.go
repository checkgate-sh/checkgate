package provider

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/hashicorp/terraform-plugin-framework-jsontypes/jsontypes"
	"github.com/hashicorp/terraform-plugin-framework/path"
	"github.com/hashicorp/terraform-plugin-framework/resource"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/planmodifier"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/stringplanmodifier"
	"github.com/hashicorp/terraform-plugin-framework/types"

	checkgate "github.com/checkgate-sh/checkgate/integrations/checkgate-go"
)

var (
	_ resource.Resource                = &segmentResource{}
	_ resource.ResourceWithConfigure   = &segmentResource{}
	_ resource.ResourceWithImportState = &segmentResource{}
)

// NewSegmentResource is the resource factory registered with the provider.
func NewSegmentResource() resource.Resource { return &segmentResource{} }

type segmentResource struct {
	client *checkgate.Client
}

type segmentModel struct {
	EnvironmentID types.String         `tfsdk:"environment_id"`
	Key           types.String         `tfsdk:"key"`
	Name          types.String         `tfsdk:"name"`
	Description   types.String         `tfsdk:"description"`
	Rules         jsontypes.Normalized `tfsdk:"rules"`
	ID            types.String         `tfsdk:"id"`
}

func (r *segmentResource) Metadata(_ context.Context, req resource.MetadataRequest, resp *resource.MetadataResponse) {
	resp.TypeName = req.ProviderTypeName + "_segment"
}

func (r *segmentResource) Schema(_ context.Context, _ resource.SchemaRequest, resp *resource.SchemaResponse) {
	requiresReplace := []planmodifier.String{stringplanmodifier.RequiresReplace()}
	resp.Schema = schema.Schema{
		MarkdownDescription: "A reusable Checkgate segment (named targeting rule set).",
		Attributes: map[string]schema.Attribute{
			"environment_id": schema.StringAttribute{
				MarkdownDescription: "Environment ID the segment lives in. Changing it forces a new resource.",
				Required:            true,
				PlanModifiers:       requiresReplace,
			},
			"key": schema.StringAttribute{
				MarkdownDescription: "Unique segment key. Changing it forces a new resource.",
				Required:            true,
				PlanModifiers:       requiresReplace,
			},
			"name": schema.StringAttribute{
				MarkdownDescription: "Human-readable segment name.",
				Required:            true,
			},
			"description": schema.StringAttribute{
				MarkdownDescription: "Optional description.",
				Optional:            true,
			},
			"rules": schema.StringAttribute{
				MarkdownDescription: "JSON array of targeting rules. Use `jsonencode([...])`.",
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

func (r *segmentResource) Configure(_ context.Context, req resource.ConfigureRequest, resp *resource.ConfigureResponse) {
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

func (r *segmentResource) modelToSegment(m *segmentModel) (*checkgate.Segment, error) {
	seg := &checkgate.Segment{Key: m.Key.ValueString(), Name: m.Name.ValueString()}
	if !m.Description.IsNull() {
		d := m.Description.ValueString()
		seg.Description = &d
	}
	if !m.Rules.IsNull() {
		if err := json.Unmarshal([]byte(m.Rules.ValueString()), &seg.Rules); err != nil {
			return nil, fmt.Errorf("invalid rules JSON: %w", err)
		}
	}
	return seg, nil
}

func (r *segmentResource) segmentToModel(envID string, seg *checkgate.Segment, m *segmentModel) {
	m.ID = types.StringValue(envID + "/" + seg.Key)
	m.EnvironmentID = types.StringValue(envID)
	m.Key = types.StringValue(seg.Key)
	m.Name = types.StringValue(seg.Name)
	m.Description = optString(seg.Description)
	if len(seg.Rules) > 0 {
		if b, err := json.Marshal(seg.Rules); err == nil {
			m.Rules = jsontypes.NewNormalizedValue(string(b))
		}
	} else if m.Rules.IsUnknown() {
		m.Rules = jsontypes.NewNormalizedNull()
	}
}

func (r *segmentResource) Create(ctx context.Context, req resource.CreateRequest, resp *resource.CreateResponse) {
	var plan segmentModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() {
		return
	}
	seg, err := r.modelToSegment(&plan)
	if err != nil {
		resp.Diagnostics.AddError("Invalid segment", err.Error())
		return
	}
	created, err := r.client.CreateSegment(ctx, plan.EnvironmentID.ValueString(), seg)
	if err != nil {
		resp.Diagnostics.AddError("Error creating segment", err.Error())
		return
	}
	r.segmentToModel(plan.EnvironmentID.ValueString(), created, &plan)
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *segmentResource) Read(ctx context.Context, req resource.ReadRequest, resp *resource.ReadResponse) {
	var state segmentModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	seg, err := r.client.GetSegment(ctx, state.EnvironmentID.ValueString(), state.Key.ValueString())
	if err != nil {
		if checkgate.NotFound(err) {
			resp.State.RemoveResource(ctx)
			return
		}
		resp.Diagnostics.AddError("Error reading segment", err.Error())
		return
	}
	r.segmentToModel(state.EnvironmentID.ValueString(), seg, &state)
	resp.Diagnostics.Append(resp.State.Set(ctx, &state)...)
}

func (r *segmentResource) Update(ctx context.Context, req resource.UpdateRequest, resp *resource.UpdateResponse) {
	var plan segmentModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() {
		return
	}
	seg, err := r.modelToSegment(&plan)
	if err != nil {
		resp.Diagnostics.AddError("Invalid segment", err.Error())
		return
	}
	updated, err := r.client.UpdateSegment(ctx, plan.EnvironmentID.ValueString(), plan.Key.ValueString(), seg)
	if err != nil {
		resp.Diagnostics.AddError("Error updating segment", err.Error())
		return
	}
	r.segmentToModel(plan.EnvironmentID.ValueString(), updated, &plan)
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *segmentResource) Delete(ctx context.Context, req resource.DeleteRequest, resp *resource.DeleteResponse) {
	var state segmentModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if err := r.client.DeleteSegment(ctx, state.EnvironmentID.ValueString(), state.Key.ValueString()); err != nil && !checkgate.NotFound(err) {
		resp.Diagnostics.AddError("Error deleting segment", err.Error())
	}
}

func (r *segmentResource) ImportState(ctx context.Context, req resource.ImportStateRequest, resp *resource.ImportStateResponse) {
	envID, key, ok := strings.Cut(req.ID, "/")
	if !ok || envID == "" || key == "" {
		resp.Diagnostics.AddError("Invalid import ID", `expected "<environment_id>/<key>"`)
		return
	}
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("environment_id"), envID)...)
	resp.Diagnostics.Append(resp.State.SetAttribute(ctx, path.Root("key"), key)...)
}
