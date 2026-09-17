package controller

import (
	"context"
	"encoding/json"
	"fmt"
	"time"

	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	"sigs.k8s.io/controller-runtime/pkg/log"

	checkgate "github.com/checkgate-sh/checkgate/integrations/checkgate-go"
	flagsv1alpha1 "github.com/checkgate-sh/checkgate-operator/api/v1alpha1"
)

const finalizer = "flags.checkgate.io/finalizer"

// FlagAPI is the slice of the Checkgate client the controller needs. Narrowing
// it to an interface lets the reconcile loop be unit-tested with a fake.
type FlagAPI interface {
	GetFlag(ctx context.Context, envID, key string) (*checkgate.Flag, error)
	CreateFlag(ctx context.Context, envID string, flag *checkgate.Flag) (*checkgate.Flag, error)
	UpdateFlag(ctx context.Context, envID, key string, flag *checkgate.Flag) (*checkgate.Flag, error)
	DeleteFlag(ctx context.Context, envID, key string) error
}

// FeatureFlagReconciler reconciles a FeatureFlag object into a Checkgate flag.
type FeatureFlagReconciler struct {
	client.Client
	Scheme *runtime.Scheme
	// NewClient builds a Checkgate API client; overridable in tests.
	NewClient func(baseURL, token string) (FlagAPI, error)
}

// +kubebuilder:rbac:groups=flags.checkgate.io,resources=featureflags,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=flags.checkgate.io,resources=featureflags/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=flags.checkgate.io,resources=featureflags/finalizers,verbs=update
// +kubebuilder:rbac:groups="",resources=secrets,verbs=get;list;watch

// Reconcile drives a FeatureFlag toward its desired state in Checkgate.
func (r *FeatureFlagReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	var ff flagsv1alpha1.FeatureFlag
	if err := r.Get(ctx, req.NamespacedName, &ff); err != nil {
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	api, err := r.buildClient(ctx, &ff)

	// Handle deletion: remove the flag from Checkgate, then drop the finalizer.
	if !ff.DeletionTimestamp.IsZero() {
		if controllerutil.ContainsFinalizer(&ff, finalizer) {
			if err == nil {
				if derr := api.DeleteFlag(ctx, ff.Spec.EnvironmentID, ff.Spec.Key); derr != nil && !checkgate.NotFound(derr) {
					logger.Error(derr, "deleting flag in Checkgate")
					return ctrl.Result{}, derr
				}
			}
			controllerutil.RemoveFinalizer(&ff, finalizer)
			if uerr := r.Update(ctx, &ff); uerr != nil {
				return ctrl.Result{}, uerr
			}
		}
		return ctrl.Result{}, nil
	}

	// Ensure our finalizer is present before we create anything remote.
	if controllerutil.AddFinalizer(&ff, finalizer) {
		if uerr := r.Update(ctx, &ff); uerr != nil {
			return ctrl.Result{}, uerr
		}
		return ctrl.Result{Requeue: true}, nil
	}

	if err != nil {
		return r.markFailed(ctx, &ff, "ClientError", err)
	}

	desired, err := specToFlag(&ff.Spec)
	if err != nil {
		return r.markFailed(ctx, &ff, "InvalidSpec", err)
	}

	// Upsert: create if missing, otherwise patch to match spec.
	_, getErr := api.GetFlag(ctx, ff.Spec.EnvironmentID, ff.Spec.Key)
	switch {
	case checkgate.NotFound(getErr):
		if _, cerr := api.CreateFlag(ctx, ff.Spec.EnvironmentID, desired); cerr != nil {
			return r.markFailed(ctx, &ff, "CreateFailed", cerr)
		}
		logger.Info("created flag in Checkgate", "key", ff.Spec.Key)
	case getErr != nil:
		return r.markFailed(ctx, &ff, "ReadFailed", getErr)
	default:
		if _, uerr := api.UpdateFlag(ctx, ff.Spec.EnvironmentID, ff.Spec.Key, desired); uerr != nil {
			return r.markFailed(ctx, &ff, "UpdateFailed", uerr)
		}
	}

	return r.markSynced(ctx, &ff)
}

// buildClient resolves the token Secret and constructs a Checkgate client.
func (r *FeatureFlagReconciler) buildClient(ctx context.Context, ff *flagsv1alpha1.FeatureFlag) (FlagAPI, error) {
	ref := ff.Spec.Server.TokenSecretRef
	ns := ref.Namespace
	if ns == "" {
		ns = ff.Namespace
	}
	secretKey := ref.Key
	if secretKey == "" {
		secretKey = "token"
	}

	var secret corev1.Secret
	if err := r.Get(ctx, types.NamespacedName{Namespace: ns, Name: ref.Name}, &secret); err != nil {
		return nil, fmt.Errorf("reading token secret %s/%s: %w", ns, ref.Name, err)
	}
	raw, ok := secret.Data[secretKey]
	if !ok || len(raw) == 0 {
		return nil, fmt.Errorf("secret %s/%s has no non-empty key %q", ns, ref.Name, secretKey)
	}

	factory := r.NewClient
	if factory == nil {
		factory = func(baseURL, token string) (FlagAPI, error) {
			return checkgate.NewClient(baseURL, token, checkgate.WithUserAgent("checkgate-operator"))
		}
	}
	return factory(ff.Spec.Server.URL, string(raw))
}

func (r *FeatureFlagReconciler) markSynced(ctx context.Context, ff *flagsv1alpha1.FeatureFlag) (ctrl.Result, error) {
	ff.Status.Synced = true
	ff.Status.ObservedGeneration = ff.Generation
	meta.SetStatusCondition(&ff.Status.Conditions, metav1.Condition{
		Type:    "Ready",
		Status:  metav1.ConditionTrue,
		Reason:  "Synced",
		Message: "Flag reconciled into Checkgate",
	})
	if err := r.Status().Update(ctx, ff); err != nil {
		return ctrl.Result{}, err
	}
	// Periodically re-reconcile to correct out-of-band drift.
	return ctrl.Result{RequeueAfter: 5 * time.Minute}, nil
}

func (r *FeatureFlagReconciler) markFailed(ctx context.Context, ff *flagsv1alpha1.FeatureFlag, reason string, cause error) (ctrl.Result, error) {
	ff.Status.Synced = false
	meta.SetStatusCondition(&ff.Status.Conditions, metav1.Condition{
		Type:    "Ready",
		Status:  metav1.ConditionFalse,
		Reason:  reason,
		Message: cause.Error(),
	})
	if err := r.Status().Update(ctx, ff); err != nil {
		return ctrl.Result{}, err
	}
	// Surface the error to controller-runtime so it backs off and retries.
	return ctrl.Result{}, cause
}

// specToFlag maps a FeatureFlagSpec to a Checkgate API flag.
func specToFlag(spec *flagsv1alpha1.FeatureFlagSpec) (*checkgate.Flag, error) {
	flag := &checkgate.Flag{
		Key:       spec.Key,
		IsEnabled: spec.Enabled,
		FlagType:  spec.FlagType,
		Tags:      spec.Tags,
	}
	if spec.RolloutPercentage != nil {
		p := *spec.RolloutPercentage
		flag.RolloutPercentage = &p
	}
	if spec.Description != "" {
		d := spec.Description
		flag.Description = &d
	}
	if spec.DefaultValue != nil {
		flag.DefaultValue = json.RawMessage(spec.DefaultValue.Raw)
	}
	if spec.Rules != nil {
		if err := json.Unmarshal(spec.Rules.Raw, &flag.Rules); err != nil {
			return nil, fmt.Errorf("spec.rules is not valid JSON: %w", err)
		}
	}
	if spec.Variants != nil {
		if err := json.Unmarshal(spec.Variants.Raw, &flag.Variants); err != nil {
			return nil, fmt.Errorf("spec.variants is not valid JSON: %w", err)
		}
	}
	return flag, nil
}

// SetupWithManager wires the reconciler into the manager.
func (r *FeatureFlagReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&flagsv1alpha1.FeatureFlag{}).
		Complete(r)
}
