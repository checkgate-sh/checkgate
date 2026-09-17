package controller

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"

	corev1 "k8s.io/api/core/v1"
	apiextensionsv1 "k8s.io/apiextensions-apiserver/pkg/apis/apiextensions/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"

	checkgate "github.com/checkgate-sh/checkgate/integrations/checkgate-go"
	flagsv1alpha1 "github.com/checkgate-sh/checkgate-operator/api/v1alpha1"
)

func testScheme(t *testing.T) *runtime.Scheme {
	t.Helper()
	s := runtime.NewScheme()
	if err := clientgoscheme.AddToScheme(s); err != nil {
		t.Fatal(err)
	}
	if err := flagsv1alpha1.AddToScheme(s); err != nil {
		t.Fatal(err)
	}
	return s
}

// recorder counts the API calls the reconciler makes to a fake Checkgate server.
type recorder struct {
	mu                       sync.Mutex
	gets, posts, patches, dels int
}

func newCheckgateServer(t *testing.T, rec *recorder, flagExists bool) *httptest.Server {
	t.Helper()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		rec.mu.Lock()
		defer rec.mu.Unlock()
		switch r.Method {
		case http.MethodGet:
			rec.gets++
			if !flagExists {
				http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
				return
			}
			json.NewEncoder(w).Encode(checkgate.Flag{Key: "my-flag", IsEnabled: true})
		case http.MethodPost:
			rec.posts++
			w.WriteHeader(http.StatusCreated)
			json.NewEncoder(w).Encode(checkgate.Flag{Key: "my-flag", IsEnabled: true})
		case http.MethodPatch:
			rec.patches++
			json.NewEncoder(w).Encode(checkgate.Flag{Key: "my-flag", IsEnabled: false})
		case http.MethodDelete:
			rec.dels++
			w.WriteHeader(http.StatusNoContent)
		}
	}))
	t.Cleanup(srv.Close)
	return srv
}

func newFF(url string, withFinalizer bool) *flagsv1alpha1.FeatureFlag {
	ff := &flagsv1alpha1.FeatureFlag{
		ObjectMeta: metav1.ObjectMeta{Name: "my-flag", Namespace: "default"},
		Spec: flagsv1alpha1.FeatureFlagSpec{
			Server: flagsv1alpha1.ServerRef{
				URL:            url,
				TokenSecretRef: flagsv1alpha1.SecretKeyRef{Name: "cg-token"},
			},
			EnvironmentID: "env1",
			Key:           "my-flag",
			Enabled:       true,
			FlagType:      "boolean",
			DefaultValue:  &apiextensionsv1.JSON{Raw: []byte("true")},
		},
	}
	if withFinalizer {
		ff.Finalizers = []string{finalizer}
	}
	return ff
}

func tokenSecret() *corev1.Secret {
	return &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{Name: "cg-token", Namespace: "default"},
		Data:       map[string][]byte{"token": []byte("tok_rw")},
	}
}

func newReconciler(s *runtime.Scheme, objs ...client.Object) *FeatureFlagReconciler {
	c := fake.NewClientBuilder().
		WithScheme(s).
		WithObjects(objs...).
		WithStatusSubresource(&flagsv1alpha1.FeatureFlag{}).
		Build()
	return &FeatureFlagReconciler{
		Client: c,
		Scheme: s,
		NewClient: func(baseURL, token string) (FlagAPI, error) {
			return checkgate.NewClient(baseURL, token)
		},
	}
}

func reconcileOnce(t *testing.T, r *FeatureFlagReconciler) ctrl.Result {
	t.Helper()
	res, err := r.Reconcile(context.Background(), ctrl.Request{
		NamespacedName: types.NamespacedName{Namespace: "default", Name: "my-flag"},
	})
	if err != nil {
		t.Fatalf("Reconcile error: %v", err)
	}
	return res
}

func TestReconcileAddsFinalizerFirst(t *testing.T) {
	s := testScheme(t)
	rec := &recorder{}
	srv := newCheckgateServer(t, rec, false)
	r := newReconciler(s, newFF(srv.URL, false), tokenSecret())

	reconcileOnce(t, r) // should only add the finalizer, no remote calls yet

	if rec.posts != 0 || rec.gets != 0 {
		t.Fatalf("expected no API calls before finalizer set, got gets=%d posts=%d", rec.gets, rec.posts)
	}
	var got flagsv1alpha1.FeatureFlag
	if err := r.Get(context.Background(), types.NamespacedName{Namespace: "default", Name: "my-flag"}, &got); err != nil {
		t.Fatal(err)
	}
	if !controllerutil.ContainsFinalizer(&got, finalizer) {
		t.Fatal("expected finalizer to be added")
	}
}

func TestReconcileCreatesMissingFlag(t *testing.T) {
	s := testScheme(t)
	rec := &recorder{}
	srv := newCheckgateServer(t, rec, false) // GET → 404 → create
	r := newReconciler(s, newFF(srv.URL, true), tokenSecret())

	reconcileOnce(t, r)

	if rec.gets != 1 || rec.posts != 1 {
		t.Fatalf("expected 1 GET + 1 POST, got gets=%d posts=%d patches=%d", rec.gets, rec.posts, rec.patches)
	}
	var got flagsv1alpha1.FeatureFlag
	if err := r.Get(context.Background(), types.NamespacedName{Namespace: "default", Name: "my-flag"}, &got); err != nil {
		t.Fatal(err)
	}
	if !got.Status.Synced {
		t.Error("expected status.synced = true")
	}
}

func TestReconcileUpdatesExistingFlag(t *testing.T) {
	s := testScheme(t)
	rec := &recorder{}
	srv := newCheckgateServer(t, rec, true) // GET → 200 → patch
	r := newReconciler(s, newFF(srv.URL, true), tokenSecret())

	reconcileOnce(t, r)

	if rec.patches != 1 || rec.posts != 0 {
		t.Fatalf("expected 1 PATCH + 0 POST, got posts=%d patches=%d", rec.posts, rec.patches)
	}
}

func TestReconcileDeleteRemovesFlagAndFinalizer(t *testing.T) {
	s := testScheme(t)
	rec := &recorder{}
	srv := newCheckgateServer(t, rec, true)
	ff := newFF(srv.URL, true)
	r := newReconciler(s, ff, tokenSecret())

	// Deleting an object that still has a finalizer sets DeletionTimestamp.
	if err := r.Delete(context.Background(), ff); err != nil {
		t.Fatal(err)
	}

	reconcileOnce(t, r)

	if rec.dels != 1 {
		t.Fatalf("expected 1 DELETE, got %d", rec.dels)
	}
	// Finalizer removed → object is now fully gone.
	var got flagsv1alpha1.FeatureFlag
	err := r.Get(context.Background(), types.NamespacedName{Namespace: "default", Name: "my-flag"}, &got)
	if err == nil {
		t.Fatal("expected FeatureFlag to be deleted after finalizer removal")
	}
}

func TestReconcileMissingSecretFails(t *testing.T) {
	s := testScheme(t)
	rec := &recorder{}
	srv := newCheckgateServer(t, rec, false)
	// No token Secret seeded.
	r := newReconciler(s, newFF(srv.URL, true))

	_, err := r.Reconcile(context.Background(), ctrl.Request{
		NamespacedName: types.NamespacedName{Namespace: "default", Name: "my-flag"},
	})
	if err == nil {
		t.Fatal("expected reconcile to fail when the token Secret is missing")
	}
	var got flagsv1alpha1.FeatureFlag
	if gerr := r.Get(context.Background(), types.NamespacedName{Namespace: "default", Name: "my-flag"}, &got); gerr != nil {
		t.Fatal(gerr)
	}
	if got.Status.Synced {
		t.Error("expected status.synced = false on failure")
	}
}
