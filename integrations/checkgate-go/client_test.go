package checkgate

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

func ptr[T any](v T) *T { return &v }

// newTestClient spins up an httptest server with the given handler and returns a
// Client pointed at it.
func newTestClient(t *testing.T, handler http.HandlerFunc) (*Client, *httptest.Server) {
	t.Helper()
	srv := httptest.NewServer(handler)
	t.Cleanup(srv.Close)
	c, err := NewClient(srv.URL, "tok_test")
	if err != nil {
		t.Fatalf("NewClient: %v", err)
	}
	return c, srv
}

func TestNewClientValidation(t *testing.T) {
	if _, err := NewClient("", "t"); err == nil {
		t.Error("expected error for empty baseURL")
	}
	if _, err := NewClient("http://x", ""); err == nil {
		t.Error("expected error for empty token")
	}
}

func TestAuthAndHeaders(t *testing.T) {
	var gotAuth, gotCSRF, gotUA string
	c, _ := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		gotAuth = r.Header.Get("Authorization")
		gotCSRF = r.Header.Get("X-Checkgate-Request")
		gotUA = r.Header.Get("User-Agent")
		json.NewEncoder(w).Encode(Flag{Key: "f"})
	})
	if _, err := c.GetFlag(context.Background(), "env1", "f"); err != nil {
		t.Fatalf("GetFlag: %v", err)
	}
	if gotAuth != "Bearer tok_test" {
		t.Errorf("Authorization = %q", gotAuth)
	}
	if gotCSRF != "true" {
		t.Errorf("X-Checkgate-Request = %q", gotCSRF)
	}
	if gotUA != "checkgate-go" {
		t.Errorf("User-Agent = %q", gotUA)
	}
}

func TestCreateFlag(t *testing.T) {
	var gotPath, gotMethod string
	var received Flag
	c, _ := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		gotPath, gotMethod = r.URL.Path, r.Method
		json.NewDecoder(r.Body).Decode(&received)
		received.Description = ptr("stored")
		w.WriteHeader(http.StatusCreated)
		json.NewEncoder(w).Encode(received)
	})

	in := &Flag{
		Key:               "new-homepage",
		IsEnabled:         true,
		RolloutPercentage: ptr(50),
		FlagType:          "boolean",
		DefaultValue:      json.RawMessage(`true`),
		Tags:              []string{"web"},
	}
	out, err := c.CreateFlag(context.Background(), "env-abc", in)
	if err != nil {
		t.Fatalf("CreateFlag: %v", err)
	}
	if gotMethod != http.MethodPost {
		t.Errorf("method = %s", gotMethod)
	}
	if gotPath != "/api/environments/env-abc/flags" {
		t.Errorf("path = %s", gotPath)
	}
	if received.Key != "new-homepage" || received.RolloutPercentage == nil || *received.RolloutPercentage != 50 {
		t.Errorf("server received unexpected flag: %+v", received)
	}
	if string(out.DefaultValue) != "true" {
		t.Errorf("default_value round-trip = %s", out.DefaultValue)
	}
}

func TestGetFlagNotFound(t *testing.T) {
	c, _ := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
	})
	_, err := c.GetFlag(context.Background(), "env1", "missing")
	if err == nil || !NotFound(err) {
		t.Fatalf("expected NotFound, got %v", err)
	}
}

func TestUpdateFlagApprovalRequired(t *testing.T) {
	c, _ := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPatch {
			t.Errorf("method = %s", r.Method)
		}
		// Environment requires approval → 202 + a change-request body.
		w.WriteHeader(http.StatusAccepted)
		json.NewEncoder(w).Encode(map[string]any{"id": 7, "status": "pending"})
	})
	_, err := c.UpdateFlag(context.Background(), "env1", "f", &Flag{Key: "f", IsEnabled: false})
	if err != ErrApprovalRequired {
		t.Fatalf("expected ErrApprovalRequired, got %v", err)
	}
}

func TestUpdateFlagApplied(t *testing.T) {
	c, _ := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(Flag{Key: "f", IsEnabled: false, FlagType: "boolean"})
	})
	out, err := c.UpdateFlag(context.Background(), "env1", "f", &Flag{Key: "f", IsEnabled: false})
	if err != nil {
		t.Fatalf("UpdateFlag: %v", err)
	}
	if out.IsEnabled {
		t.Error("expected is_enabled=false")
	}
}

func TestDeleteFlag(t *testing.T) {
	var gotMethod string
	c, _ := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		gotMethod = r.Method
		w.WriteHeader(http.StatusNoContent)
	})
	if err := c.DeleteFlag(context.Background(), "env1", "f"); err != nil {
		t.Fatalf("DeleteFlag: %v", err)
	}
	if gotMethod != http.MethodDelete {
		t.Errorf("method = %s", gotMethod)
	}
}

func TestServerErrorSurfacesStatus(t *testing.T) {
	c, _ := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, "boom", http.StatusInternalServerError)
	})
	_, err := c.GetFlag(context.Background(), "env1", "f")
	apiErr, ok := err.(*APIError)
	if !ok {
		t.Fatalf("expected *APIError, got %T", err)
	}
	if apiErr.StatusCode != http.StatusInternalServerError {
		t.Errorf("status = %d", apiErr.StatusCode)
	}
}

func TestSegmentCRUD(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("POST /api/environments/env1/segments", func(w http.ResponseWriter, r *http.Request) {
		var s Segment
		json.NewDecoder(r.Body).Decode(&s)
		s.ID = "seg-1"
		w.WriteHeader(http.StatusCreated)
		json.NewEncoder(w).Encode(s)
	})
	mux.HandleFunc("GET /api/environments/env1/segments/internal", func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(Segment{ID: "seg-1", Key: "internal", Name: "Internal"})
	})
	mux.HandleFunc("PATCH /api/environments/env1/segments/internal", func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(Segment{ID: "seg-1", Key: "internal", Name: "Renamed"})
	})
	mux.HandleFunc("DELETE /api/environments/env1/segments/internal", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusNoContent)
	})
	c, _ := newTestClient(t, mux.ServeHTTP)
	ctx := context.Background()

	created, err := c.CreateSegment(ctx, "env1", &Segment{Key: "internal", Name: "Internal"})
	if err != nil || created.ID != "seg-1" {
		t.Fatalf("CreateSegment: %v (%+v)", err, created)
	}
	got, err := c.GetSegment(ctx, "env1", "internal")
	if err != nil || got.Name != "Internal" {
		t.Fatalf("GetSegment: %v (%+v)", err, got)
	}
	upd, err := c.UpdateSegment(ctx, "env1", "internal", &Segment{Key: "internal", Name: "Renamed"})
	if err != nil || upd.Name != "Renamed" {
		t.Fatalf("UpdateSegment: %v (%+v)", err, upd)
	}
	if err := c.DeleteSegment(ctx, "env1", "internal"); err != nil {
		t.Fatalf("DeleteSegment: %v", err)
	}
}

// ReplaceFlag must send every clearable field explicitly — an emptied rule
// list or a removed description has to clear the stored value, which the
// omitempty encoding UpdateFlag uses cannot express.
func TestReplaceFlagSendsClearedFields(t *testing.T) {
	var body map[string]json.RawMessage
	c, _ := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPatch {
			t.Errorf("method = %s", r.Method)
		}
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		json.NewEncoder(w).Encode(Flag{Key: "f", FlagType: "boolean"})
	})
	if _, err := c.ReplaceFlag(context.Background(), "env1", "f", &Flag{Key: "f"}); err != nil {
		t.Fatalf("ReplaceFlag: %v", err)
	}
	want := map[string]string{
		"is_enabled":         "false",
		"rollout_percentage": "null",
		"description":        "null",
		"rules":              "[]",
		"default_value":      "null",
		"disabled_value":     "null",
		"variants":           "[]",
		"prerequisites":      "[]",
		"tags":               "[]",
		"owner_email":        "null",
	}
	for field, value := range want {
		if got, ok := body[field]; !ok || string(got) != value {
			t.Errorf("%s = %s (present %v), want %s", field, got, ok, value)
		}
	}
	if _, ok := body["flag_type"]; ok {
		t.Error("an empty flag_type keeps the stored type and must be omitted")
	}
	if _, ok := body["key"]; ok {
		t.Error("the key is in the URL, not the body")
	}
}

// UpdateFlag stays a partial update: empty fields are left out.
func TestUpdateFlagOmitsEmptyFields(t *testing.T) {
	var body map[string]json.RawMessage
	c, _ := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		json.NewDecoder(r.Body).Decode(&body)
		json.NewEncoder(w).Encode(Flag{Key: "f"})
	})
	if _, err := c.UpdateFlag(context.Background(), "env1", "f", &Flag{Key: "f", IsEnabled: true}); err != nil {
		t.Fatalf("UpdateFlag: %v", err)
	}
	for _, field := range []string{"rules", "description", "tags"} {
		if _, ok := body[field]; ok {
			t.Errorf("%s should be omitted from a partial update", field)
		}
	}
}
