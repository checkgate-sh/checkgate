package provider

import (
	"context"
	"encoding/json"

	"github.com/hashicorp/terraform-plugin-framework-jsontypes/jsontypes"
	"github.com/hashicorp/terraform-plugin-framework/diag"
	"github.com/hashicorp/terraform-plugin-framework/types"

	checkgate "github.com/checkgate-sh/checkgate/integrations/checkgate-go"
)

// modelToFlag builds an API Flag from Terraform plan/state.
func modelToFlag(ctx context.Context, m *flagModel) (*checkgate.Flag, diag.Diagnostics) {
	var diags diag.Diagnostics

	flag := &checkgate.Flag{
		Key:       m.Key.ValueString(),
		IsEnabled: m.IsEnabled.ValueBool(),
		FlagType:  m.FlagType.ValueString(),
	}

	if !m.RolloutPercentage.IsNull() {
		p := int(m.RolloutPercentage.ValueInt64())
		flag.RolloutPercentage = &p
	}
	if !m.Description.IsNull() {
		d := m.Description.ValueString()
		flag.Description = &d
	}
	if !m.OwnerEmail.IsNull() {
		e := m.OwnerEmail.ValueString()
		flag.OwnerEmail = &e
	}
	if !m.DefaultValue.IsNull() {
		flag.DefaultValue = json.RawMessage(m.DefaultValue.ValueString())
	}
	if !m.DisabledValue.IsNull() {
		flag.DisabledValue = json.RawMessage(m.DisabledValue.ValueString())
	}

	if !m.Tags.IsNull() {
		diags.Append(m.Tags.ElementsAs(ctx, &flag.Tags, false)...)
	}
	if !m.Rules.IsNull() {
		if err := json.Unmarshal([]byte(m.Rules.ValueString()), &flag.Rules); err != nil {
			diags.AddError("Invalid rules JSON", err.Error())
		}
	}
	if !m.Variants.IsNull() {
		if err := json.Unmarshal([]byte(m.Variants.ValueString()), &flag.Variants); err != nil {
			diags.AddError("Invalid variants JSON", err.Error())
		}
	}
	if !m.Prerequisites.IsNull() {
		if err := json.Unmarshal([]byte(m.Prerequisites.ValueString()), &flag.Prerequisites); err != nil {
			diags.AddError("Invalid prerequisites JSON", err.Error())
		}
	}

	return flag, diags
}

// flagToModel writes an API Flag back into a Terraform model. JSON-valued
// attributes use jsontypes.Normalized, so semantic (not textual) equality keeps
// re-marshalled server output from producing spurious diffs. Optional list/JSON
// fields that the server omits are only cleared when the model didn't set them,
// preserving the user's declared value.
func flagToModel(_ context.Context, envID string, flag *checkgate.Flag, m *flagModel) diag.Diagnostics {
	var diags diag.Diagnostics

	m.ID = types.StringValue(envID + "/" + flag.Key)
	m.EnvironmentID = types.StringValue(envID)
	m.Key = types.StringValue(flag.Key)
	m.IsEnabled = types.BoolValue(flag.IsEnabled)
	m.FlagType = types.StringValue(flag.FlagType)

	if flag.RolloutPercentage != nil {
		m.RolloutPercentage = types.Int64Value(int64(*flag.RolloutPercentage))
	} else {
		m.RolloutPercentage = types.Int64Null()
	}
	m.Description = optString(flag.Description)
	m.OwnerEmail = optString(flag.OwnerEmail)
	m.DefaultValue = rawToNormalized(flag.DefaultValue)
	m.DisabledValue = rawToNormalized(flag.DisabledValue)

	if len(flag.Tags) > 0 {
		list, d := types.ListValueFrom(context.Background(), types.StringType, flag.Tags)
		diags.Append(d...)
		m.Tags = list
	} else if m.Tags.IsUnknown() {
		m.Tags = types.ListNull(types.StringType)
	}

	m.Rules = sliceToNormalized(flag.Rules, m.Rules, &diags)
	m.Variants = sliceToNormalized(flag.Variants, m.Variants, &diags)
	m.Prerequisites = sliceToNormalized(flag.Prerequisites, m.Prerequisites, &diags)

	return diags
}

func optString(s *string) types.String {
	if s == nil {
		return types.StringNull()
	}
	return types.StringValue(*s)
}

func rawToNormalized(raw json.RawMessage) jsontypes.Normalized {
	if len(raw) == 0 {
		return jsontypes.NewNormalizedNull()
	}
	return jsontypes.NewNormalizedValue(string(raw))
}

// sliceToNormalized marshals a slice back to JSON. An empty slice keeps the
// prior value (null stays null) so an unset optional attribute doesn't flip to
// "[]" and churn the plan.
func sliceToNormalized[T any](items []T, prior jsontypes.Normalized, diags *diag.Diagnostics) jsontypes.Normalized {
	if len(items) == 0 {
		if prior.IsUnknown() {
			return jsontypes.NewNormalizedNull()
		}
		return prior
	}
	b, err := json.Marshal(items)
	if err != nil {
		diags.AddError("Error encoding flag field", err.Error())
		return prior
	}
	return jsontypes.NewNormalizedValue(string(b))
}
