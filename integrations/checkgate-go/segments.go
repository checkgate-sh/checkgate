package checkgate

import (
	"context"
	"fmt"
	"net/http"
	"net/url"
)

// Segment is a reusable named set of targeting rules.
type Segment struct {
	ID          string          `json:"id,omitempty"`
	Key         string          `json:"key"`
	Name        string          `json:"name"`
	Description *string         `json:"description,omitempty"`
	Rules       []TargetingRule `json:"rules,omitempty"`
	CreatedAt   string          `json:"created_at,omitempty"`
}

func segmentsPath(envID string) string {
	return fmt.Sprintf("/api/environments/%s/segments", url.PathEscape(envID))
}

func segmentPath(envID, key string) string {
	return fmt.Sprintf("/api/environments/%s/segments/%s", url.PathEscape(envID), url.PathEscape(key))
}

// GetSegment fetches a segment by key. Use NotFound(err) to detect absence.
func (c *Client) GetSegment(ctx context.Context, envID, key string) (*Segment, error) {
	var s Segment
	if _, err := c.do(ctx, http.MethodGet, segmentPath(envID, key), nil, &s); err != nil {
		return nil, err
	}
	return &s, nil
}

// CreateSegment creates a segment and returns the stored result.
func (c *Client) CreateSegment(ctx context.Context, envID string, seg *Segment) (*Segment, error) {
	var out Segment
	if _, err := c.do(ctx, http.MethodPost, segmentsPath(envID), seg, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// segmentPatch is the mutable subset of a segment (key is immutable).
type segmentPatch struct {
	Name        *string         `json:"name,omitempty"`
	Description *string         `json:"description,omitempty"`
	Rules       []TargetingRule `json:"rules,omitempty"`
}

// UpdateSegment applies a partial update to a segment: a nil Description or
// empty Rules is omitted and keeps the stored value. To make the segment match
// a desired state exactly, use ReplaceSegment.
func (c *Client) UpdateSegment(ctx context.Context, envID, key string, seg *Segment) (*Segment, error) {
	return c.patchSegment(ctx, envID, key, segmentPatch{Name: &seg.Name, Description: seg.Description, Rules: seg.Rules})
}

// ReplaceSegment sets the segment's name, description, and rules to the
// values in seg: a nil Description or empty Rules clears the stored value.
// This is what declarative callers (Terraform) need.
func (c *Client) ReplaceSegment(ctx context.Context, envID, key string, seg *Segment) (*Segment, error) {
	return c.patchSegment(ctx, envID, key, segmentReplacement{
		Name:        seg.Name,
		Description: seg.Description,
		Rules:       nonNil(seg.Rules),
	})
}

func (c *Client) patchSegment(ctx context.Context, envID, key string, body any) (*Segment, error) {
	var out Segment
	if _, err := c.do(ctx, http.MethodPatch, segmentPath(envID, key), body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// segmentReplacement is the PATCH body ReplaceSegment sends: nothing is
// omitted, so a nil description is sent as null and empty rules as [].
type segmentReplacement struct {
	Name        string          `json:"name"`
	Description *string         `json:"description"`
	Rules       []TargetingRule `json:"rules"`
}

// DeleteSegment removes a segment.
func (c *Client) DeleteSegment(ctx context.Context, envID, key string) error {
	_, err := c.do(ctx, http.MethodDelete, segmentPath(envID, key), nil, nil)
	return err
}
