use crate::hashing::murmurhash3_x86_32;
use crate::store::FlagStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::num::NonZeroU64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    #[default]
    Equals,
    NotEquals,
    Contains,
    StartsWith,
    EndsWith,
    /// Numeric comparisons. Both the user attribute and the rule values are parsed
    /// as f64; a rule matches if the comparison holds against **any** listed value.
    /// A non-numeric attribute or value never matches.
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FlagType {
    #[default]
    Boolean,
    String,
    Integer,
    Json,
}

/// The value returned from a flag evaluation. Stored as untagged JSON so it
/// round-trips through the JSONB column and SSE stream without a type wrapper.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(untagged)]
pub enum FlagValue {
    Bool(bool),
    Str(String),
    Int(i64),
    Json(serde_json::Value),
    #[default]
    Null,
}

/// Full evaluation result — includes both the on/off decision and the resolved value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalResult {
    pub enabled: bool,
    pub value: FlagValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TargetingRule {
    /// Concrete rule fields. Defaults allow omitting them when `segment_key` is set.
    #[serde(default)]
    pub attribute: String,
    #[serde(default)]
    pub operator: Operator,
    #[serde(default)]
    pub values: Vec<String>,
    /// When set, this rule references a named segment. The server expands the segment's
    /// rules inline before broadcasting to SDK clients, so the evaluator never sees this
    /// field in production. It is preserved here for DB round-trips and dashboard display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_key: Option<String>,
    /// Optional value returned when this rule matches (non-boolean flags).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<FlagValue>,
}

/// One entry in a weighted multivariate distribution (see `Flag::variants`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WeightedVariant {
    /// Relative weight. Weights need not sum to 100 — they are normalized against
    /// their total (e.g. `[60, 30, 10]` behaves identically to `[6, 3, 1]`).
    pub weight: u32,
    pub value: FlagValue,
}

/// One prerequisite a flag depends on (see `Flag::prerequisites`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Prerequisite {
    pub flag_key: String,
    /// Value the prerequisite flag must resolve to. `None` means "just needs to
    /// be enabled" — the common case for boolean prerequisites, where there's no
    /// specific variant value to check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_value: Option<FlagValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Flag {
    pub key: String,
    pub is_enabled: bool,
    pub rollout_percentage: Option<u32>, // 0 to 100
    pub description: Option<String>,
    #[serde(default)]
    pub rules: Vec<TargetingRule>,
    /// Variant type — defaults to Boolean for backward compatibility.
    #[serde(default)]
    pub flag_type: FlagType,
    /// Value returned when the flag is enabled and no targeting rule overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<FlagValue>,
    /// Value returned when `is_enabled` is false or the user is outside the rollout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_value: Option<FlagValue>,
    /// Weighted distribution across multiple variant values (e.g. a 60/30/10 A/B/C
    /// split) — the basis for experimentation. When non-empty, an evaluation that is
    /// enabled and matches no targeting rule is bucketed across these weighted
    /// variants instead of returning `default_value`. Bucketing uses a hash salted
    /// independently from the rollout-percentage gate, so variant assignment is
    /// uniform among users who pass the gate regardless of the rollout percentage.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<WeightedVariant>,
    /// Other flags this flag depends on. If any prerequisite is not satisfied,
    /// this flag evaluates as disabled — even if its own `is_enabled`/rules/
    /// rollout would otherwise say yes. Checked before rules and rollout.
    /// Evaluated recursively (a prerequisite can itself have prerequisites), with
    /// a depth guard against cycles or excessively deep chains. A prerequisite
    /// referencing a flag that doesn't exist is treated as unsatisfied (fails closed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prerequisites: Vec<Prerequisite>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserContext {
    pub key: String, // User identifier strictly for hashing
    pub attributes: HashMap<String, String>,
}

fn rule_matches(rule: &TargetingRule, user_context: &UserContext) -> bool {
    let user_attr = match user_context.attributes.get(&rule.attribute) {
        Some(v) => v,
        None => {
            // A missing attribute satisfies NotEquals ("the user is definitely not X")
            // but fails all other operators which require the value to be present.
            //
            // NOTE: This means users without a targeted attribute will pass a NotEquals
            // rule, which can be surprising. For example, a rule "org not_equals evil_corp"
            // will admit anonymous users who have no "org" attribute at all. Add a
            // separate Equals rule for the expected attribute values if you want to
            // restrict access to known-good values only.
            return rule.operator == Operator::NotEquals && !rule.values.is_empty();
        }
    };

    match rule.operator {
        Operator::Equals => rule.values.iter().any(|v| v == user_attr),
        Operator::NotEquals => !rule.values.iter().any(|v| v == user_attr),
        Operator::Contains => rule.values.iter().any(|v| user_attr.contains(v.as_str())),
        Operator::StartsWith => rule
            .values
            .iter()
            .any(|v| user_attr.starts_with(v.as_str())),
        Operator::EndsWith => rule.values.iter().any(|v| user_attr.ends_with(v.as_str())),
        Operator::GreaterThan => numeric_cmp(user_attr, &rule.values, |a, b| a > b),
        Operator::GreaterThanOrEqual => numeric_cmp(user_attr, &rule.values, |a, b| a >= b),
        Operator::LessThan => numeric_cmp(user_attr, &rule.values, |a, b| a < b),
        Operator::LessThanOrEqual => numeric_cmp(user_attr, &rule.values, |a, b| a <= b),
    }
}

/// Parse the user attribute and each rule value as f64 and test the comparison.
/// Returns true if the comparison holds against any value. Non-numeric inputs
/// never match (they are silently skipped rather than erroring).
fn numeric_cmp(attr: &str, values: &[String], cmp: fn(f64, f64) -> bool) -> bool {
    let a: f64 = match attr.trim().parse() {
        Ok(x) => x,
        Err(_) => return false,
    };
    values
        .iter()
        .any(|v| v.trim().parse::<f64>().map(|b| cmp(a, b)).unwrap_or(false))
}

/// Returns the value to use when the flag is on but has no explicit default.
/// Boolean flags fall back to `true` for backward compatibility with flags that
/// predate `default_value`. Non-boolean flags fall back to `Null` to avoid
/// returning a value of the wrong type.
fn enabled_default(flag: &Flag) -> FlagValue {
    match flag.flag_type {
        FlagType::Boolean => FlagValue::Bool(true),
        _ => FlagValue::Null,
    }
}

fn disabled_result(flag: &Flag) -> EvalResult {
    EvalResult {
        enabled: false,
        value: flag
            .disabled_value
            .clone()
            .unwrap_or(FlagValue::Bool(false)),
    }
}

/// Deterministically buckets a user into one of `variants` proportional to their
/// weights. Salted independently from the rollout-percentage hash (`:variant` suffix)
/// so variant assignment doesn't correlate with which users pass the rollout gate.
/// Zero total weight (all-zero or empty) falls back to the first variant.
fn pick_weighted_variant(
    flag_key: &str,
    user_key: &str,
    variants: &[WeightedVariant],
) -> FlagValue {
    let total: u64 = variants
        .iter()
        .map(|v| u64::from(v.weight))
        .fold(0u64, u64::saturating_add);

    // Carrying the total as a `NonZeroU64` makes the "all weights zero" case a
    // type-level guarantee: `u64 % NonZeroU64` cannot divide by zero, so the
    // remainder below has no panic path at all.
    let Some(total_weight) = NonZeroU64::new(total) else {
        return variants
            .first()
            .map(|v| v.value.clone())
            .unwrap_or(FlagValue::Null);
    };

    let hash_key = format!("{}:{}:variant", flag_key, user_key);
    let hash_val = u64::from(murmurhash3_x86_32(hash_key.as_bytes(), 0));
    let bucket = if total <= u64::from(u32::MAX) {
        hash_val % total_weight
    } else {
        // u128 covers the complete u32 hash times any u64 total without overflow.
        let scaled = u128::from(hash_val).saturating_mul(u128::from(total)) >> 32;
        u64::try_from(scaled).unwrap_or(0)
    };

    let mut cumulative = 0u64;
    for variant in variants {
        cumulative = cumulative.saturating_add(u64::from(variant.weight));
        if bucket < cumulative {
            return variant.value.clone();
        }
    }
    // Unreachable given bucket < total_weight == final cumulative, but guard anyway.
    variants
        .last()
        .map(|v| v.value.clone())
        .unwrap_or(FlagValue::Null)
}

/// Prerequisite chains deeper than this are treated as unsatisfied (fail closed).
/// Guards against cycles (e.g. A requires B, B requires A) as well as
/// misconfigured, excessively deep chains — without this, a cycle would recurse
/// forever rather than erroring.
const MAX_PREREQUISITE_DEPTH: usize = 10;

/// Evaluate a flag and return both the on/off result and the resolved variant value.
/// Use this for non-boolean flags (string / integer / JSON variants). `store` is
/// used to resolve the flag's prerequisites (if any) — pass the same store the
/// flag itself was looked up from.
pub fn evaluate_variant(flag: &Flag, user_context: &UserContext, store: &FlagStore) -> EvalResult {
    evaluate_variant_at_depth(flag, user_context, store, 0)
}

fn evaluate_variant_at_depth(
    flag: &Flag,
    user_context: &UserContext,
    store: &FlagStore,
    depth: usize,
) -> EvalResult {
    if !flag.is_enabled {
        return disabled_result(flag);
    }

    if !prerequisites_satisfied(flag, user_context, store, depth) {
        return disabled_result(flag);
    }

    // Targeting rules — first match wins; per-rule variant overrides the flag default.
    for rule in &flag.rules {
        if rule_matches(rule, user_context) {
            let value = rule
                .variant
                .clone()
                .or_else(|| flag.default_value.clone())
                .unwrap_or_else(|| enabled_default(flag));
            return EvalResult {
                enabled: true,
                value,
            };
        }
    }

    // Rollout bucket check
    if let Some(percentage) = flag.rollout_percentage {
        if percentage == 0 {
            return disabled_result(flag);
        }
        if percentage < 100 {
            let hash_key = format!("{}:{}", flag.key, user_context.key);
            let hash_val = murmurhash3_x86_32(hash_key.as_bytes(), 0);
            if hash_val % 100 >= percentage {
                return disabled_result(flag);
            }
        }
    }

    let value = if !flag.variants.is_empty() {
        pick_weighted_variant(&flag.key, &user_context.key, &flag.variants)
    } else {
        flag.default_value
            .clone()
            .unwrap_or_else(|| enabled_default(flag))
    };

    EvalResult {
        enabled: true,
        value,
    }
}

/// Checks that every prerequisite of `flag` is satisfied, recursively evaluating each
/// prerequisite flag via `store`. Fails closed: a prerequisite referencing a flag that
/// no longer exists, or a chain deeper than `MAX_PREREQUISITE_DEPTH` (guards against
/// cycles, e.g. A requires B, B requires A), is treated as unsatisfied.
fn prerequisites_satisfied(
    flag: &Flag,
    user_context: &UserContext,
    store: &FlagStore,
    depth: usize,
) -> bool {
    if flag.prerequisites.is_empty() {
        return true;
    }
    if depth >= MAX_PREREQUISITE_DEPTH {
        return false;
    }

    flag.prerequisites.iter().all(|prereq| {
        let Some(prereq_flag) = store.get_flag(&prereq.flag_key) else {
            return false;
        };
        let result = evaluate_variant_at_depth(
            prereq_flag.as_ref(),
            user_context,
            store,
            depth.saturating_add(1),
        );
        match &prereq.required_value {
            Some(required) => result.enabled && &result.value == required,
            None => result.enabled,
        }
    })
}

/// Evaluate a flag and return a simple boolean. Delegates to `evaluate_variant`.
/// Existing callers are unaffected.
pub fn evaluate(flag: &Flag, user_context: &UserContext, store: &FlagStore) -> bool {
    evaluate_variant(flag, user_context, store).enabled
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn bool_flag(
        key: &str,
        enabled: bool,
        rollout: Option<u32>,
        rules: Vec<TargetingRule>,
    ) -> Flag {
        Flag {
            key: key.into(),
            is_enabled: enabled,
            rollout_percentage: rollout,
            description: None,
            rules,
            flag_type: FlagType::Boolean,
            default_value: None,
            disabled_value: None,
            variants: vec![],
            prerequisites: vec![],
        }
    }

    #[test]
    fn large_equal_weights_distribute_across_both_variants() {
        let variants = vec![
            WeightedVariant {
                weight: u32::MAX,
                value: FlagValue::Str("a".into()),
            },
            WeightedVariant {
                weight: u32::MAX,
                value: FlagValue::Str("b".into()),
            },
        ];
        let count_a = (0..10_000)
            .filter(|i| {
                pick_weighted_variant("large", &format!("user{i}"), &variants)
                    == FlagValue::Str("a".into())
            })
            .count();
        assert!(
            (4500..5500).contains(&count_a),
            "equal large weights: {count_a} assignments to a"
        );
    }

    #[test]
    fn test_flag_disabled() {
        let flag = bool_flag("new_ui", false, None, vec![]);
        let ctx = UserContext {
            key: "user123".into(),
            attributes: HashMap::new(),
        };
        assert!(!evaluate(&flag, &ctx, &FlagStore::new()));
    }

    #[test]
    fn test_flag_rollout() {
        let flag = bool_flag("new_ui", true, Some(50), vec![]);

        let mut trues = 0;
        let mut falses = 0;

        for i in 0..1000 {
            let ctx = UserContext {
                key: format!("user{}", i),
                attributes: HashMap::new(),
            };
            if evaluate(&flag, &ctx, &FlagStore::new()) {
                trues += 1;
            } else {
                falses += 1;
            }
        }

        // With 1000 users and 50% rollout via hashing, it should be approximately 50-50
        assert!(trues > 450 && trues < 550);
        assert!(falses > 450 && falses < 550);
    }

    fn weighted_variant_flag(variants: Vec<WeightedVariant>) -> Flag {
        Flag {
            key: "checkout-experiment".into(),
            is_enabled: true,
            rollout_percentage: None,
            description: None,
            rules: vec![],
            flag_type: FlagType::String,
            default_value: Some(FlagValue::Str("should-not-be-used".into())),
            disabled_value: None,
            variants,
            prerequisites: vec![],
        }
    }

    #[test]
    fn test_weighted_variants_distribution_converges_to_weights() {
        let flag = weighted_variant_flag(vec![
            WeightedVariant {
                weight: 60,
                value: FlagValue::Str("control".into()),
            },
            WeightedVariant {
                weight: 30,
                value: FlagValue::Str("treatment-a".into()),
            },
            WeightedVariant {
                weight: 10,
                value: FlagValue::Str("treatment-b".into()),
            },
        ]);

        let mut counts: HashMap<String, u32> = HashMap::new();
        for i in 0..10_000 {
            let ctx = UserContext {
                key: format!("user{}", i),
                attributes: HashMap::new(),
            };
            let result = evaluate_variant(&flag, &ctx, &FlagStore::new());
            assert!(result.enabled);
            if let FlagValue::Str(v) = result.value {
                *counts.entry(v).or_insert(0) += 1;
            } else {
                panic!("expected string variant value");
            }
        }

        let control = *counts.get("control").unwrap_or(&0);
        let treatment_a = *counts.get("treatment-a").unwrap_or(&0);
        let treatment_b = *counts.get("treatment-b").unwrap_or(&0);

        // Within a few percentage points of the configured 60/30/10 split.
        assert!(
            (5500..6500).contains(&control),
            "control={control} outside expected range"
        );
        assert!(
            (2500..3500).contains(&treatment_a),
            "treatment_a={treatment_a} outside expected range"
        );
        assert!(
            (500..1500).contains(&treatment_b),
            "treatment_b={treatment_b} outside expected range"
        );
        assert_eq!(control + treatment_a + treatment_b, 10_000);
    }

    #[test]
    fn test_weighted_variants_are_sticky_per_user() {
        let flag = weighted_variant_flag(vec![
            WeightedVariant {
                weight: 50,
                value: FlagValue::Str("a".into()),
            },
            WeightedVariant {
                weight: 50,
                value: FlagValue::Str("b".into()),
            },
        ]);
        let ctx = UserContext {
            key: "consistent-user".into(),
            attributes: HashMap::new(),
        };
        let first = evaluate_variant(&flag, &ctx, &FlagStore::new()).value;
        for _ in 0..50 {
            assert_eq!(
                evaluate_variant(&flag, &ctx, &FlagStore::new()).value,
                first
            );
        }
    }

    #[test]
    fn test_weighted_variants_unnormalized_weights_still_work() {
        // Weights [6, 3, 1] should behave identically to [60, 30, 10].
        let flag = weighted_variant_flag(vec![
            WeightedVariant {
                weight: 6,
                value: FlagValue::Str("control".into()),
            },
            WeightedVariant {
                weight: 3,
                value: FlagValue::Str("treatment-a".into()),
            },
            WeightedVariant {
                weight: 1,
                value: FlagValue::Str("treatment-b".into()),
            },
        ]);
        let mut counts: HashMap<String, u32> = HashMap::new();
        for i in 0..10_000 {
            let ctx = UserContext {
                key: format!("user{}", i),
                attributes: HashMap::new(),
            };
            if let FlagValue::Str(v) = evaluate_variant(&flag, &ctx, &FlagStore::new()).value {
                *counts.entry(v).or_insert(0) += 1;
            }
        }
        let control = *counts.get("control").unwrap_or(&0);
        assert!(
            (5500..6500).contains(&control),
            "control={control} outside expected range"
        );
    }

    #[test]
    fn test_weighted_variants_empty_falls_back_to_default_value() {
        let flag = weighted_variant_flag(vec![]);
        let ctx = UserContext {
            key: "u".into(),
            attributes: HashMap::new(),
        };
        let result = evaluate_variant(&flag, &ctx, &FlagStore::new());
        assert!(result.enabled);
        assert_eq!(result.value, FlagValue::Str("should-not-be-used".into()));
    }

    #[test]
    fn test_weighted_variants_zero_total_weight_falls_back_to_first() {
        let flag = weighted_variant_flag(vec![
            WeightedVariant {
                weight: 0,
                value: FlagValue::Str("only-option".into()),
            },
            WeightedVariant {
                weight: 0,
                value: FlagValue::Str("unreachable".into()),
            },
        ]);
        let ctx = UserContext {
            key: "u".into(),
            attributes: HashMap::new(),
        };
        assert_eq!(
            evaluate_variant(&flag, &ctx, &FlagStore::new()).value,
            FlagValue::Str("only-option".into())
        );
    }

    #[test]
    fn test_weighted_variants_bypassed_by_matching_rule() {
        let mut flag = weighted_variant_flag(vec![WeightedVariant {
            weight: 100,
            value: FlagValue::Str("experiment-variant".into()),
        }]);
        flag.rules.push(TargetingRule {
            attribute: "plan".into(),
            operator: Operator::Equals,
            values: vec!["enterprise".into()],
            variant: Some(FlagValue::Str("enterprise-override".into())),
            segment_key: None,
        });
        let result = evaluate_variant(&flag, &ctx_with("plan", "enterprise"), &FlagStore::new());
        assert_eq!(result.value, FlagValue::Str("enterprise-override".into()));
    }

    #[test]
    fn test_weighted_variants_bypassed_when_disabled() {
        let mut flag = weighted_variant_flag(vec![WeightedVariant {
            weight: 100,
            value: FlagValue::Str("experiment-variant".into()),
        }]);
        flag.is_enabled = false;
        flag.disabled_value = Some(FlagValue::Str("off".into()));
        let ctx = UserContext {
            key: "u".into(),
            attributes: HashMap::new(),
        };
        let result = evaluate_variant(&flag, &ctx, &FlagStore::new());
        assert!(!result.enabled);
        assert_eq!(result.value, FlagValue::Str("off".into()));
    }

    #[test]
    fn test_weighted_variants_respect_rollout_gate() {
        // 0% rollout means nobody reaches variant selection — always disabled_value.
        let mut flag = weighted_variant_flag(vec![WeightedVariant {
            weight: 100,
            value: FlagValue::Str("experiment-variant".into()),
        }]);
        flag.rollout_percentage = Some(0);
        flag.disabled_value = Some(FlagValue::Str("off".into()));
        let ctx = UserContext {
            key: "u".into(),
            attributes: HashMap::new(),
        };
        let result = evaluate_variant(&flag, &ctx, &FlagStore::new());
        assert!(!result.enabled);
        assert_eq!(result.value, FlagValue::Str("off".into()));
    }

    fn flag_with_prerequisites(
        key: &str,
        is_enabled: bool,
        prerequisites: Vec<Prerequisite>,
    ) -> Flag {
        Flag {
            key: key.into(),
            is_enabled,
            rollout_percentage: None,
            description: None,
            rules: vec![],
            flag_type: FlagType::Boolean,
            default_value: None,
            disabled_value: None,
            variants: vec![],
            prerequisites,
        }
    }

    #[test]
    fn test_prerequisite_satisfied_enables_dependent_flag() {
        let store = FlagStore::new();
        store.upsert_flag(flag_with_prerequisites("base-feature", true, vec![]));

        let dependent = flag_with_prerequisites(
            "advanced-feature",
            true,
            vec![Prerequisite {
                flag_key: "base-feature".into(),
                required_value: None,
            }],
        );
        let ctx = UserContext {
            key: "u".into(),
            attributes: HashMap::new(),
        };
        assert!(evaluate(&dependent, &ctx, &store));
    }

    #[test]
    fn test_prerequisite_unsatisfied_disables_dependent_flag() {
        let store = FlagStore::new();
        store.upsert_flag(flag_with_prerequisites("base-feature", false, vec![]));

        // The dependent's own config says "enabled, and this rule matches" —
        // proving the prerequisite gate takes priority over the dependent's own
        // rules, not just a simple is_enabled check.
        let mut dependent = flag_with_prerequisites(
            "advanced-feature",
            true,
            vec![Prerequisite {
                flag_key: "base-feature".into(),
                required_value: None,
            }],
        );
        dependent.rules.push(TargetingRule {
            attribute: "plan".into(),
            operator: Operator::Equals,
            values: vec!["enterprise".into()],
            variant: None,
            segment_key: None,
        });

        assert!(!evaluate(
            &dependent,
            &ctx_with("plan", "enterprise"),
            &store
        ));
    }

    #[test]
    fn test_prerequisite_required_value_match_and_mismatch() {
        let store = FlagStore::new();
        store.upsert_flag(Flag {
            key: "theme".into(),
            is_enabled: true,
            rollout_percentage: None,
            description: None,
            rules: vec![],
            flag_type: FlagType::String,
            default_value: Some(FlagValue::Str("dark".into())),
            disabled_value: None,
            variants: vec![],
            prerequisites: vec![],
        });

        let matching = flag_with_prerequisites(
            "dark-mode-extras",
            true,
            vec![Prerequisite {
                flag_key: "theme".into(),
                required_value: Some(FlagValue::Str("dark".into())),
            }],
        );
        let mismatching = flag_with_prerequisites(
            "dark-mode-extras",
            true,
            vec![Prerequisite {
                flag_key: "theme".into(),
                required_value: Some(FlagValue::Str("light".into())),
            }],
        );
        let ctx = UserContext {
            key: "u".into(),
            attributes: HashMap::new(),
        };
        assert!(evaluate(&matching, &ctx, &store));
        assert!(!evaluate(&mismatching, &ctx, &store));
    }

    #[test]
    fn test_prerequisite_missing_flag_fails_closed() {
        let store = FlagStore::new(); // "base-feature" never registered
        let dependent = flag_with_prerequisites(
            "advanced-feature",
            true,
            vec![Prerequisite {
                flag_key: "base-feature".into(),
                required_value: None,
            }],
        );
        let ctx = UserContext {
            key: "u".into(),
            attributes: HashMap::new(),
        };
        assert!(!evaluate(&dependent, &ctx, &store));
    }

    #[test]
    fn test_prerequisite_chain_two_hops() {
        let store = FlagStore::new();
        store.upsert_flag(flag_with_prerequisites("infra-ready", true, vec![]));
        store.upsert_flag(flag_with_prerequisites(
            "base-feature",
            true,
            vec![Prerequisite {
                flag_key: "infra-ready".into(),
                required_value: None,
            }],
        ));
        let top = flag_with_prerequisites(
            "advanced-feature",
            true,
            vec![Prerequisite {
                flag_key: "base-feature".into(),
                required_value: None,
            }],
        );
        let ctx = UserContext {
            key: "u".into(),
            attributes: HashMap::new(),
        };
        assert!(evaluate(&top, &ctx, &store));

        // Break the bottom of the chain — should transitively disable the top.
        store.upsert_flag(flag_with_prerequisites("infra-ready", false, vec![]));
        assert!(!evaluate(&top, &ctx, &store));
    }

    #[test]
    fn test_prerequisite_cycle_fails_closed_without_hanging() {
        let store = FlagStore::new();
        store.upsert_flag(flag_with_prerequisites(
            "flag-a",
            true,
            vec![Prerequisite {
                flag_key: "flag-b".into(),
                required_value: None,
            }],
        ));
        store.upsert_flag(flag_with_prerequisites(
            "flag-b",
            true,
            vec![Prerequisite {
                flag_key: "flag-a".into(),
                required_value: None,
            }],
        ));
        let flag_a = store.get_flag("flag-a").unwrap();
        let ctx = UserContext {
            key: "u".into(),
            attributes: HashMap::new(),
        };
        // Must terminate (not stack overflow / hang) and fail closed.
        assert!(!evaluate(flag_a.as_ref(), &ctx, &store));
    }

    fn num_rule(attr: &str, op: Operator, values: &[&str]) -> TargetingRule {
        TargetingRule {
            attribute: attr.into(),
            operator: op,
            values: values.iter().map(|s| s.to_string()).collect(),
            variant: None,
            segment_key: None,
        }
    }

    fn ctx_with(attr: &str, val: &str) -> UserContext {
        let mut attributes = HashMap::new();
        attributes.insert(attr.to_string(), val.to_string());
        UserContext {
            key: "u".into(),
            attributes,
        }
    }

    #[test]
    fn test_numeric_operators() {
        let gt = bool_flag(
            "g",
            true,
            Some(0),
            vec![num_rule("age", Operator::GreaterThan, &["18"])],
        );
        assert!(evaluate(&gt, &ctx_with("age", "21"), &FlagStore::new()));
        assert!(!evaluate(&gt, &ctx_with("age", "18"), &FlagStore::new())); // strict
        assert!(!evaluate(&gt, &ctx_with("age", "12"), &FlagStore::new()));

        let gte = bool_flag(
            "g",
            true,
            Some(0),
            vec![num_rule("age", Operator::GreaterThanOrEqual, &["18"])],
        );
        assert!(evaluate(&gte, &ctx_with("age", "18"), &FlagStore::new()));
        assert!(!evaluate(&gte, &ctx_with("age", "17"), &FlagStore::new()));

        let lt = bool_flag(
            "g",
            true,
            Some(0),
            vec![num_rule("score", Operator::LessThan, &["100"])],
        );
        assert!(evaluate(&lt, &ctx_with("score", "99.5"), &FlagStore::new())); // floats supported
        assert!(!evaluate(&lt, &ctx_with("score", "100"), &FlagStore::new()));

        let lte = bool_flag(
            "g",
            true,
            Some(0),
            vec![num_rule("score", Operator::LessThanOrEqual, &["100"])],
        );
        assert!(evaluate(&lte, &ctx_with("score", "100"), &FlagStore::new()));
        assert!(!evaluate(
            &lte,
            &ctx_with("score", "100.01"),
            &FlagStore::new()
        ));
    }

    #[test]
    fn test_numeric_non_numeric_never_matches() {
        let gt = bool_flag(
            "g",
            true,
            Some(0),
            vec![num_rule("age", Operator::GreaterThan, &["18"])],
        );
        // Non-numeric attribute value must not match.
        assert!(!evaluate(
            &gt,
            &ctx_with("age", "eighteen"),
            &FlagStore::new()
        ));
        // Missing attribute must not match (only NotEquals matches on absence).
        assert!(!evaluate(
            &gt,
            &UserContext {
                key: "u".into(),
                attributes: HashMap::new(),
            },
            &FlagStore::new()
        ));
    }

    #[test]
    fn test_operator_serde_snake_case() {
        // Operators are the dashboard/DB wire contract — they must stay snake_case.
        assert_eq!(
            serde_json::to_string(&Operator::GreaterThanOrEqual).unwrap(),
            "\"greater_than_or_equal\""
        );
        let parsed: Operator = serde_json::from_str("\"less_than\"").unwrap();
        assert_eq!(parsed, Operator::LessThan);
    }

    #[test]
    fn test_not_equals_missing_attribute_matches() {
        let flag = bool_flag(
            "org_gate",
            true,
            Some(0),
            vec![TargetingRule {
                attribute: "org".into(),
                operator: Operator::NotEquals,
                values: vec!["evil_corp".into()],
                variant: None,
                segment_key: None,
            }],
        );
        let ctx = UserContext {
            key: "anon".into(),
            attributes: HashMap::new(),
        };
        assert!(evaluate(&flag, &ctx, &FlagStore::new()));
    }

    #[test]
    fn test_not_equals_present_and_matching_value_does_not_match() {
        let flag = bool_flag(
            "org_gate",
            true,
            Some(0),
            vec![TargetingRule {
                attribute: "org".into(),
                operator: Operator::NotEquals,
                values: vec!["evil_corp".into()],
                variant: None,
                segment_key: None,
            }],
        );
        let mut attrs = HashMap::new();
        attrs.insert("org".into(), "evil_corp".into());
        let ctx = UserContext {
            key: "villain".into(),
            attributes: attrs,
        };
        assert!(!evaluate(&flag, &ctx, &FlagStore::new()));
    }

    #[test]
    fn test_rollout_percentage_above_100_treated_as_full_rollout() {
        let flag = bool_flag("bad_pct", true, Some(150), vec![]);
        let ctx = UserContext {
            key: "anyone".into(),
            attributes: HashMap::new(),
        };
        assert!(evaluate(&flag, &ctx, &FlagStore::new()));
    }

    #[test]
    fn test_flag_rules_match() {
        let flag = bool_flag(
            "beta_feature",
            true,
            Some(0),
            vec![TargetingRule {
                attribute: "email".into(),
                operator: Operator::EndsWith,
                values: vec!["@checkgate.com".into()],
                variant: None,
                segment_key: None,
            }],
        );
        let mut attrs = HashMap::new();
        attrs.insert("email".into(), "test@checkgate.com".into());
        let ctx = UserContext {
            key: "employee".into(),
            attributes: attrs,
        };
        assert!(evaluate(&flag, &ctx, &FlagStore::new()));
    }

    #[test]
    fn test_evaluate_variant_string_flag() {
        let flag = Flag {
            key: "theme".into(),
            is_enabled: true,
            rollout_percentage: None,
            description: None,
            rules: vec![],
            flag_type: FlagType::String,
            default_value: Some(FlagValue::Str("dark".into())),
            disabled_value: Some(FlagValue::Str("light".into())),
            variants: vec![],
            prerequisites: vec![],
        };
        let ctx = UserContext {
            key: "user1".into(),
            attributes: HashMap::new(),
        };
        let result = evaluate_variant(&flag, &ctx, &FlagStore::new());
        assert!(result.enabled);
        assert_eq!(result.value, FlagValue::Str("dark".into()));
    }

    #[test]
    fn test_evaluate_variant_disabled_returns_disabled_value() {
        let flag = Flag {
            key: "theme".into(),
            is_enabled: false,
            rollout_percentage: None,
            description: None,
            rules: vec![],
            flag_type: FlagType::String,
            default_value: Some(FlagValue::Str("dark".into())),
            disabled_value: Some(FlagValue::Str("light".into())),
            variants: vec![],
            prerequisites: vec![],
        };
        let ctx = UserContext {
            key: "user1".into(),
            attributes: HashMap::new(),
        };
        let result = evaluate_variant(&flag, &ctx, &FlagStore::new());
        assert!(!result.enabled);
        assert_eq!(result.value, FlagValue::Str("light".into()));
    }

    #[test]
    fn test_evaluate_variant_per_rule_variant() {
        let flag = Flag {
            key: "checkout".into(),
            is_enabled: true,
            rollout_percentage: None,
            description: None,
            rules: vec![TargetingRule {
                attribute: "plan".into(),
                operator: Operator::Equals,
                values: vec!["enterprise".into()],
                variant: Some(FlagValue::Str("v3".into())),
                segment_key: None,
            }],
            flag_type: FlagType::String,
            default_value: Some(FlagValue::Str("v2".into())),
            disabled_value: None,
            variants: vec![],
            prerequisites: vec![],
        };

        let mut attrs = HashMap::new();
        attrs.insert("plan".into(), "enterprise".into());
        let ctx = UserContext {
            key: "bigcorp".into(),
            attributes: attrs,
        };
        let result = evaluate_variant(&flag, &ctx, &FlagStore::new());
        assert!(result.enabled);
        assert_eq!(result.value, FlagValue::Str("v3".into()));

        // Non-enterprise user gets default value
        let ctx2 = UserContext {
            key: "regular".into(),
            attributes: HashMap::new(),
        };
        let result2 = evaluate_variant(&flag, &ctx2, &FlagStore::new());
        assert!(result2.enabled);
        assert_eq!(result2.value, FlagValue::Str("v2".into()));
    }

    #[test]
    fn test_old_boolean_flags_deserialize_without_new_fields() {
        let json = r#"{"key":"legacy","is_enabled":true,"rollout_percentage":null,"description":null,"rules":[]}"#;
        let flag: Flag = serde_json::from_str(json).unwrap();
        assert_eq!(flag.flag_type, FlagType::Boolean);
        assert!(flag.default_value.is_none());
        assert!(flag.disabled_value.is_none());
        assert!(evaluate(
            &flag,
            &UserContext {
                key: "u".into(),
                attributes: HashMap::new()
            },
            &FlagStore::new()
        ));
    }

    // --- Additional coverage: string operators, boundaries, value types ---

    fn ctx_attr(key: &str, attr: &str, val: &str) -> UserContext {
        let mut attributes = HashMap::new();
        attributes.insert(attr.to_string(), val.to_string());
        UserContext {
            key: key.into(),
            attributes,
        }
    }

    fn string_rule(attribute: &str, operator: Operator, values: &[&str]) -> TargetingRule {
        TargetingRule {
            attribute: attribute.into(),
            operator,
            values: values.iter().map(|s| s.to_string()).collect(),
            segment_key: None,
            variant: None,
        }
    }

    #[test]
    fn test_operator_contains() {
        let flag = bool_flag(
            "f",
            true,
            Some(0), // rollout 0 → only a rule match can enable
            vec![string_rule("email", Operator::Contains, &["@acme."])],
        );
        let store = FlagStore::new();
        assert!(evaluate(
            &flag,
            &ctx_attr("u", "email", "bob@acme.com"),
            &store
        ));
        assert!(!evaluate(
            &flag,
            &ctx_attr("u", "email", "bob@other.com"),
            &store
        ));
    }

    #[test]
    fn test_operator_starts_with() {
        let flag = bool_flag(
            "f",
            true,
            Some(0),
            vec![string_rule("region", Operator::StartsWith, &["eu-"])],
        );
        let store = FlagStore::new();
        assert!(evaluate(
            &flag,
            &ctx_attr("u", "region", "eu-west-1"),
            &store
        ));
        assert!(!evaluate(
            &flag,
            &ctx_attr("u", "region", "us-east-1"),
            &store
        ));
    }

    #[test]
    fn test_equals_matches_any_value_in_list() {
        let flag = bool_flag(
            "f",
            true,
            Some(0),
            vec![string_rule(
                "plan",
                Operator::Equals,
                &["pro", "enterprise"],
            )],
        );
        let store = FlagStore::new();
        assert!(evaluate(
            &flag,
            &ctx_attr("u", "plan", "enterprise"),
            &store
        ));
        assert!(evaluate(&flag, &ctx_attr("u", "plan", "pro"), &store));
        assert!(!evaluate(&flag, &ctx_attr("u", "plan", "free"), &store));
    }

    #[test]
    fn test_first_matching_rule_wins() {
        // Two rules whose variants differ: the earlier match must be returned.
        let mut flag = bool_flag("f", true, None, vec![]);
        flag.flag_type = FlagType::String;
        flag.rules = vec![
            TargetingRule {
                attribute: "plan".into(),
                operator: Operator::Equals,
                values: vec!["pro".into()],
                segment_key: None,
                variant: Some(FlagValue::Str("first".into())),
            },
            TargetingRule {
                attribute: "plan".into(),
                operator: Operator::Equals,
                values: vec!["pro".into()],
                segment_key: None,
                variant: Some(FlagValue::Str("second".into())),
            },
        ];
        let res = evaluate_variant(&flag, &ctx_attr("u", "plan", "pro"), &FlagStore::new());
        assert_eq!(res.value, FlagValue::Str("first".into()));
    }

    #[test]
    fn test_rollout_zero_disables_and_hundred_enables_all() {
        let store = FlagStore::new();
        let zero = bool_flag("z", true, Some(0), vec![]);
        let hundred = bool_flag("h", true, Some(100), vec![]);
        for i in 0..200 {
            let ctx = UserContext {
                key: format!("user{i}"),
                attributes: HashMap::new(),
            };
            assert!(!evaluate(&zero, &ctx, &store), "0% must never enable");
            assert!(evaluate(&hundred, &ctx, &store), "100% must always enable");
        }
    }

    #[test]
    fn test_enabled_boolean_default_is_true_nonbool_is_null() {
        let store = FlagStore::new();
        let ctx = UserContext {
            key: "u".into(),
            attributes: HashMap::new(),
        };

        // Boolean flag, enabled, no default_value → true (backward compatible).
        let b = bool_flag("b", true, None, vec![]);
        let rb = evaluate_variant(&b, &ctx, &store);
        assert!(rb.enabled);
        assert_eq!(rb.value, FlagValue::Bool(true));

        // Non-boolean flag, enabled, no default_value/variants → Null value.
        let mut s = bool_flag("s", true, None, vec![]);
        s.flag_type = FlagType::String;
        let rs = evaluate_variant(&s, &ctx, &store);
        assert!(rs.enabled);
        assert_eq!(rs.value, FlagValue::Null);
    }

    #[test]
    fn test_disabled_flag_returns_disabled_value_or_false() {
        let store = FlagStore::new();
        let ctx = UserContext {
            key: "u".into(),
            attributes: HashMap::new(),
        };

        // No disabled_value → Bool(false).
        let plain = bool_flag("p", false, None, vec![]);
        assert_eq!(
            evaluate_variant(&plain, &ctx, &store).value,
            FlagValue::Bool(false)
        );

        // Explicit disabled_value is honored.
        let mut custom = bool_flag("c", false, None, vec![]);
        custom.flag_type = FlagType::String;
        custom.disabled_value = Some(FlagValue::Str("off".into()));
        let r = evaluate_variant(&custom, &ctx, &store);
        assert!(!r.enabled);
        assert_eq!(r.value, FlagValue::Str("off".into()));
    }

    #[test]
    fn test_integer_and_json_default_values_resolve() {
        let store = FlagStore::new();
        let ctx = UserContext {
            key: "u".into(),
            attributes: HashMap::new(),
        };

        let mut int_flag = bool_flag("max", true, None, vec![]);
        int_flag.flag_type = FlagType::Integer;
        int_flag.default_value = Some(FlagValue::Int(42));
        assert_eq!(
            evaluate_variant(&int_flag, &ctx, &store).value,
            FlagValue::Int(42)
        );

        let json = serde_json::json!({"mode": "dark"});
        let mut json_flag = bool_flag("cfg", true, None, vec![]);
        json_flag.flag_type = FlagType::Json;
        json_flag.default_value = Some(FlagValue::Json(json.clone()));
        assert_eq!(
            evaluate_variant(&json_flag, &ctx, &store).value,
            FlagValue::Json(json)
        );
    }
}
