package checkgate

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/url"
)

// TargetingRule is one condition in a flag's rule list. Complex/nested fields
// are kept as raw JSON so this client never has to track the full evaluation
// schema; callers that manage rules typically pass them through verbatim.
type TargetingRule struct {
	Attribute  string          `json:"attribute,omitempty"`
	Operator   string          `json:"operator,omitempty"`
	Values     []string        `json:"values,omitempty"`
	SegmentKey string          `json:"segment_key,omitempty"`
	Variant    json.RawMessage `json:"variant,omitempty"`
}

// WeightedVariant is one bucket in a weighted multivariate distribution.
type WeightedVariant struct {
	Weight int             `json:"weight"`
	Value  json.RawMessage `json:"value"`
}

// Prerequisite is another flag this flag depends on.
type Prerequisite struct {
	FlagKey       string          `json:"flag_key"`
	RequiredValue json.RawMessage `json:"required_value,omitempty"`
}

// Flag mirrors the Checkgate flag resource. Polymorphic values (default_value,
// disabled_value) are json.RawMessage so any of bool/string/int/JSON round-trips
// losslessly. Pointer/omitempty fields distinguish "unset" from a zero value.
type Flag struct {
	Key               string            `json:"key"`
	IsEnabled         bool              `json:"is_enabled"`
	RolloutPercentage *int              `json:"rollout_percentage,omitempty"`
	Description       *string           `json:"description,omitempty"`
	Rules             []TargetingRule   `json:"rules,omitempty"`
	FlagType          string            `json:"flag_type,omitempty"`
	DefaultValue      json.RawMessage   `json:"default_value,omitempty"`
	DisabledValue     json.RawMessage   `json:"disabled_value,omitempty"`
	Variants          []WeightedVariant `json:"variants,omitempty"`
	Prerequisites     []Prerequisite    `json:"prerequisites,omitempty"`
	Tags              []string          `json:"tags,omitempty"`
	OwnerEmail        *string           `json:"owner_email,omitempty"`
}

// ErrApprovalRequired is returned by UpdateFlag when the environment has
// require_approval set: the PATCH was captured as a pending change request
// (HTTP 202) instead of applying, so an infrastructure-as-code apply cannot
// complete synchronously.
var ErrApprovalRequired = errors.New("checkgate: change requires approval (environment has require_approval enabled); apply cannot complete synchronously")

func flagsPath(envID string) string {
	return fmt.Sprintf("/api/environments/%s/flags", url.PathEscape(envID))
}

func flagPath(envID, key string) string {
	return fmt.Sprintf("/api/environments/%s/flags/%s", url.PathEscape(envID), url.PathEscape(key))
}

// GetFlag fetches a flag by key. Use NotFound(err) to detect a missing flag.
func (c *Client) GetFlag(ctx context.Context, envID, key string) (*Flag, error) {
	var f Flag
	if _, err := c.do(ctx, http.MethodGet, flagPath(envID, key), nil, &f); err != nil {
		return nil, err
	}
	return &f, nil
}

// CreateFlag creates a flag in the environment and returns the stored result.
func (c *Client) CreateFlag(ctx context.Context, envID string, flag *Flag) (*Flag, error) {
	var out Flag
	if _, err := c.do(ctx, http.MethodPost, flagsPath(envID), flag, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// UpdateFlag applies a partial update (PATCH) to a flag. Fields left empty
// in patch are omitted and keep their stored value — so UpdateFlag can never
// clear rules, variants, prerequisites, tags, or a nullable field. To make the
// flag match a desired state exactly, use ReplaceFlag. Returns
// ErrApprovalRequired if the server queued the change for approval (HTTP 202).
func (c *Client) UpdateFlag(ctx context.Context, envID, key string, patch *Flag) (*Flag, error) {
	return c.patchFlag(ctx, envID, key, patch)
}

// ReplaceFlag sets every mutable field of a flag to its value in flag: an
// empty slice or nil pointer clears the stored value instead of keeping it.
// This is what declarative callers (Terraform, the Kubernetes operator) need.
// FlagType is the exception — left empty, the stored type is kept. Returns
// ErrApprovalRequired if the server queued the change for approval (HTTP 202).
func (c *Client) ReplaceFlag(ctx context.Context, envID, key string, flag *Flag) (*Flag, error) {
	return c.patchFlag(ctx, envID, key, newFlagReplacement(flag))
}

func (c *Client) patchFlag(ctx context.Context, envID, key string, body any) (*Flag, error) {
	var out Flag
	status, err := c.do(ctx, http.MethodPatch, flagPath(envID, key), body, &out)
	if err != nil {
		return nil, err
	}
	if status == http.StatusAccepted {
		return nil, ErrApprovalRequired
	}
	return &out, nil
}

// flagReplacement is the PATCH body ReplaceFlag sends. Unlike Flag, nothing
// clearable is omitted: nil slices are sent as [] and nil values as null.
type flagReplacement struct {
	IsEnabled         bool              `json:"is_enabled"`
	RolloutPercentage *int              `json:"rollout_percentage"`
	Description       *string           `json:"description"`
	Rules             []TargetingRule   `json:"rules"`
	FlagType          string            `json:"flag_type,omitempty"`
	DefaultValue      json.RawMessage   `json:"default_value"`
	DisabledValue     json.RawMessage   `json:"disabled_value"`
	Variants          []WeightedVariant `json:"variants"`
	Prerequisites     []Prerequisite    `json:"prerequisites"`
	Tags              []string          `json:"tags"`
	OwnerEmail        *string           `json:"owner_email"`
}

func newFlagReplacement(f *Flag) flagReplacement {
	return flagReplacement{
		IsEnabled:         f.IsEnabled,
		RolloutPercentage: f.RolloutPercentage,
		Description:       f.Description,
		Rules:             nonNil(f.Rules),
		FlagType:          f.FlagType,
		DefaultValue:      f.DefaultValue,
		DisabledValue:     f.DisabledValue,
		Variants:          nonNil(f.Variants),
		Prerequisites:     nonNil(f.Prerequisites),
		Tags:              nonNil(f.Tags),
		OwnerEmail:        f.OwnerEmail,
	}
}

// nonNil turns a nil slice into an empty one, which encodes as [] not null.
func nonNil[T any](s []T) []T {
	if s == nil {
		return []T{}
	}
	return s
}

// DeleteFlag removes a flag. Deleting a missing flag returns a NotFound error.
func (c *Client) DeleteFlag(ctx context.Context, envID, key string) error {
	_, err := c.do(ctx, http.MethodDelete, flagPath(envID, key), nil, nil)
	return err
}
