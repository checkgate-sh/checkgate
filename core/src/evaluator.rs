use crate::hashing::murmurhash3_x86_32_concat;
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
#[cfg(test)]
fn pick_weighted_variant(
    flag_key: &str,
    user_key: &str,
    variants: &[WeightedVariant],
) -> FlagValue {
    weighted_variant_index(flag_key, user_key, variants)
        .and_then(|i| variants.get(i))
        .map(|v| v.value.clone())
        .unwrap_or(FlagValue::Null)
}

/// The index of the variant [`pick_weighted_variant`] returns, or `None` when
/// `variants` is empty.
fn weighted_variant_index(
    flag_key: &str,
    user_key: &str,
    variants: &[WeightedVariant],
) -> Option<usize> {
    if variants.is_empty() {
        return None;
    }
    let total: u64 = variants
        .iter()
        .map(|v| u64::from(v.weight))
        .fold(0u64, u64::saturating_add);

    // Carrying the total as a `NonZeroU64` makes the "all weights zero" case a
    // type-level guarantee: `u64 % NonZeroU64` cannot divide by zero, so the
    // remainder below has no panic path at all.
    let Some(total_weight) = NonZeroU64::new(total) else {
        return Some(0);
    };

    // Hashes the bytes of `"{flag_key}:{user_key}:variant"` without building
    // that string (see `murmurhash3_x86_32_concat`).
    let hash_val = u64::from(murmurhash3_x86_32_concat(
        &[flag_key.as_bytes(), b":", user_key.as_bytes(), b":variant"],
        0,
    ));
    let bucket = if total <= u64::from(u32::MAX) {
        hash_val % total_weight
    } else {
        // u128 covers the complete u32 hash times any u64 total without overflow.
        let scaled = u128::from(hash_val).saturating_mul(u128::from(total)) >> 32;
        u64::try_from(scaled).unwrap_or(0)
    };

    let mut cumulative = 0u64;
    for (index, variant) in variants.iter().enumerate() {
        cumulative = cumulative.saturating_add(u64::from(variant.weight));
        if bucket < cumulative {
            return Some(index);
        }
    }
    // Unreachable given bucket < total_weight == final cumulative, but guard anyway.
    Some(variants.len().saturating_sub(1))
}

/// The user's rollout bucket for `flag`, in `0..100`. A user is inside a
/// rollout of `p` percent when their bucket is below `p`.
fn rollout_bucket(flag_key: &str, user_key: &str) -> u32 {
    // Hashes the bytes of `"{flag_key}:{user_key}"` without building that string.
    murmurhash3_x86_32_concat(&[flag_key.as_bytes(), b":", user_key.as_bytes()], 0) % 100
}

/// Prerequisite chains deeper than this are treated as unsatisfied (fail closed).
/// Guards against cycles (e.g. A requires B, B requires A) as well as
/// misconfigured, excessively deep chains — without this, a cycle would recurse
/// forever rather than erroring.
const MAX_PREREQUISITE_DEPTH: usize = 10;

/// Prerequisite evaluations allowed within one top-level evaluation; once
/// spent, remaining prerequisites count as unsatisfied (fail closed, like the
/// depth guard). The depth guard alone does not bound the work: prerequisites
/// that are all satisfied never short-circuit, so a chain of flags that each
/// list the next one N times costs N^depth evaluations — 8 duplicates over 10
/// levels is ~134 million, minutes of CPU in every SDK for one `isEnabled`.
/// Real configurations use a handful; 1 000 caps the worst case at roughly a
/// hundred microseconds. The server also rejects such configurations on write
/// (duplicate or excessive prerequisites); this bound protects SDKs from data
/// saved before that check, or written by other means.
const MAX_PREREQUISITE_EVALUATIONS: u32 = 1_000;

/// Why an evaluation resolved the way it did. The vocabulary follows the
/// [OpenFeature resolution reasons](https://openfeature.dev/specification/types#resolution-reason),
/// plus the two outcomes OpenFeature has no standard word for.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum Reason {
    /// The flag is switched off (`is_enabled: false`).
    Disabled,
    /// A prerequisite was not satisfied, so the flag resolved as disabled.
    PrerequisiteFailed,
    /// A targeting rule matched (see [`Explanation::rule_index`]).
    TargetingMatch,
    /// The user's rollout bucket is outside the rollout percentage, so the
    /// flag resolved as disabled (see [`Explanation::rollout_bucket`]).
    RolloutExcluded,
    /// The outcome was decided by hashing the user key: the user passed a
    /// partial rollout, or was assigned a weighted variant.
    Split,
    /// The flag has targeting rules, none matched, and the user got the
    /// flag's default value.
    Default,
    /// The flag has no rules, partial rollout or variants: every user gets
    /// the same value.
    Static,
}

/// How one prerequisite of the explained flag was judged.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PrerequisiteOutcome {
    /// The prerequisite flag resolved as required.
    Satisfied,
    /// The prerequisite flag resolved, but not as required.
    Unsatisfied,
    /// No flag with that key exists (fails closed).
    NotFound,
    /// The depth or evaluation limit was reached before it could be
    /// evaluated (fails closed).
    LimitExceeded,
}

/// One prerequisite of the explained flag, in the order they were checked.
/// Checking stops at the first prerequisite that is not satisfied, so later
/// ones are absent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct PrerequisiteCheck {
    pub flag_key: String,
    pub outcome: PrerequisiteOutcome,
    /// The value the prerequisite must resolve to; `None` means "enabled".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_value: Option<FlagValue>,
    /// What the prerequisite resolved to; `None` when it was not evaluated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<EvalResult>,
}

/// An evaluation together with the path that produced it: the same result
/// [`evaluate_variant`] returns, from the same code, plus why.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct Explanation {
    pub enabled: bool,
    pub value: FlagValue,
    pub reason: Reason,
    /// Index into `Flag::rules` of the rule that matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_index: Option<usize>,
    /// The user's rollout bucket (`0..100`) when a partial rollout
    /// (`0 < rollout_percentage < 100`) was checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollout_bucket: Option<u32>,
    /// Index into `Flag::variants` of the weighted variant assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_index: Option<usize>,
    /// The flag's own prerequisites as they were checked (not those of
    /// prerequisites further down the chain).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prerequisites: Vec<PrerequisiteCheck>,
}

impl Explanation {
    /// The plain result, as [`evaluate_variant`] returns it.
    #[must_use]
    pub fn result(&self) -> EvalResult {
        EvalResult {
            enabled: self.enabled,
            value: self.value.clone(),
        }
    }
}

/// Observes the decisions one evaluation makes. [`evaluate_variant`] runs with
/// [`NoTrace`], whose empty methods compile away, so the hot path is the same
/// machine code it was before explanations existed; [`explain`] runs the very
/// same function with a recorder. One evaluator, two views of it.
trait Trace {
    fn reason(&mut self, _reason: Reason) {}
    fn rule(&mut self, _index: usize) {}
    fn rollout_bucket(&mut self, _bucket: u32) {}
    fn variant(&mut self, _index: usize) {}
    /// Only called for the top-level flag's own prerequisites.
    fn prerequisite(&mut self, _check: PrerequisiteCheck) {}
    /// Whether [`Trace::prerequisite`] does anything; lets the caller skip
    /// building the check (it clones values) when nobody records it.
    fn records_prerequisites(&self) -> bool {
        false
    }
}

struct NoTrace;
impl Trace for NoTrace {}

#[derive(Default)]
struct Recorder {
    reason: Option<Reason>,
    rule_index: Option<usize>,
    rollout_bucket: Option<u32>,
    variant_index: Option<usize>,
    prerequisites: Vec<PrerequisiteCheck>,
}

impl Trace for Recorder {
    fn reason(&mut self, reason: Reason) {
        self.reason = Some(reason);
    }
    fn rule(&mut self, index: usize) {
        self.rule_index = Some(index);
    }
    fn rollout_bucket(&mut self, bucket: u32) {
        self.rollout_bucket = Some(bucket);
    }
    fn variant(&mut self, index: usize) {
        self.variant_index = Some(index);
    }
    fn prerequisite(&mut self, check: PrerequisiteCheck) {
        self.prerequisites.push(check);
    }
    fn records_prerequisites(&self) -> bool {
        true
    }
}

/// Evaluate a flag and return both the on/off result and the resolved variant value.
/// Use this for non-boolean flags (string / integer / JSON variants). `store` is
/// used to resolve the flag's prerequisites (if any) — pass the same store the
/// flag itself was looked up from.
pub fn evaluate_variant(flag: &Flag, user_context: &UserContext, store: &FlagStore) -> EvalResult {
    let mut budget = MAX_PREREQUISITE_EVALUATIONS;
    evaluate_variant_at_depth(flag, user_context, store, 0, &mut budget, &mut NoTrace)
}

/// Evaluate a flag and report why it resolved as it did: the reason, the
/// matched rule, the rollout bucket, the weighted variant and the outcome of
/// each prerequisite. The result is the one [`evaluate_variant`] returns for
/// the same inputs, computed by the same code. Costs more than
/// `evaluate_variant` (it records the path), so use it for debugging and
/// detailed SDK APIs, not as the default evaluation.
#[must_use]
pub fn explain(flag: &Flag, user_context: &UserContext, store: &FlagStore) -> Explanation {
    let mut budget = MAX_PREREQUISITE_EVALUATIONS;
    let mut recorder = Recorder::default();
    let result =
        evaluate_variant_at_depth(flag, user_context, store, 0, &mut budget, &mut recorder);
    Explanation {
        enabled: result.enabled,
        value: result.value,
        // Every return path of the evaluator reports a reason; `Disabled` is
        // the fail-closed answer should one ever be added without it.
        reason: recorder.reason.unwrap_or(Reason::Disabled),
        rule_index: recorder.rule_index,
        rollout_bucket: recorder.rollout_bucket,
        variant_index: recorder.variant_index,
        prerequisites: recorder.prerequisites,
    }
}

fn evaluate_variant_at_depth<T: Trace>(
    flag: &Flag,
    user_context: &UserContext,
    store: &FlagStore,
    depth: usize,
    budget: &mut u32,
    trace: &mut T,
) -> EvalResult {
    if !flag.is_enabled {
        trace.reason(Reason::Disabled);
        return disabled_result(flag);
    }

    if !prerequisites_satisfied(flag, user_context, store, depth, budget, trace) {
        trace.reason(Reason::PrerequisiteFailed);
        return disabled_result(flag);
    }

    // Targeting rules — first match wins; per-rule variant overrides the flag default.
    for (index, rule) in flag.rules.iter().enumerate() {
        if rule_matches(rule, user_context) {
            trace.reason(Reason::TargetingMatch);
            trace.rule(index);
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
    let mut hashed = false;
    if let Some(percentage) = flag.rollout_percentage {
        if percentage == 0 {
            trace.reason(Reason::RolloutExcluded);
            return disabled_result(flag);
        }
        if percentage < 100 {
            let bucket = rollout_bucket(&flag.key, &user_context.key);
            trace.rollout_bucket(bucket);
            if bucket >= percentage {
                trace.reason(Reason::RolloutExcluded);
                return disabled_result(flag);
            }
            hashed = true;
        }
    }

    let value =
        if let Some(index) = weighted_variant_index(&flag.key, &user_context.key, &flag.variants) {
            trace.variant(index);
            hashed = true;
            flag.variants
                .get(index)
                .map(|v| v.value.clone())
                .unwrap_or(FlagValue::Null)
        } else {
            flag.default_value
                .clone()
                .unwrap_or_else(|| enabled_default(flag))
        };
    trace.reason(if hashed {
        Reason::Split
    } else if flag.rules.is_empty() {
        Reason::Static
    } else {
        Reason::Default
    });

    EvalResult {
        enabled: true,
        value,
    }
}

/// Checks that every prerequisite of `flag` is satisfied, recursively evaluating each
/// prerequisite flag via `store`. Fails closed: a prerequisite referencing a flag that
/// no longer exists, a chain deeper than `MAX_PREREQUISITE_DEPTH` (guards against
/// cycles, e.g. A requires B, B requires A), or a prerequisite reached after the
/// evaluation's `MAX_PREREQUISITE_EVALUATIONS` budget is spent, is treated as
/// unsatisfied. Stops at the first unsatisfied prerequisite.
///
/// `trace` sees only this flag's own prerequisites; flags further down the
/// chain are evaluated untraced.
fn prerequisites_satisfied<T: Trace>(
    flag: &Flag,
    user_context: &UserContext,
    store: &FlagStore,
    depth: usize,
    budget: &mut u32,
    trace: &mut T,
) -> bool {
    if flag.prerequisites.is_empty() {
        return true;
    }
    let record = trace.records_prerequisites();
    let report = |trace: &mut T,
                  prereq: &Prerequisite,
                  outcome: PrerequisiteOutcome,
                  result: Option<&EvalResult>| {
        if record {
            trace.prerequisite(PrerequisiteCheck {
                flag_key: prereq.flag_key.clone(),
                outcome,
                required_value: prereq.required_value.clone(),
                result: result.cloned(),
            });
        }
    };
    if depth >= MAX_PREREQUISITE_DEPTH {
        if let Some(first) = flag.prerequisites.first() {
            report(trace, first, PrerequisiteOutcome::LimitExceeded, None);
        }
        return false;
    }

    for prereq in &flag.prerequisites {
        let Some(remaining) = budget.checked_sub(1) else {
            report(trace, prereq, PrerequisiteOutcome::LimitExceeded, None);
            return false;
        };
        *budget = remaining;
        let Some(prereq_flag) = store.get_flag(&prereq.flag_key) else {
            report(trace, prereq, PrerequisiteOutcome::NotFound, None);
            return false;
        };
        let result = evaluate_variant_at_depth(
            prereq_flag.as_ref(),
            user_context,
            store,
            depth.saturating_add(1),
            budget,
            &mut NoTrace,
        );
        let satisfied = match &prereq.required_value {
            Some(required) => result.enabled && &result.value == required,
            None => result.enabled,
        };
        let outcome = if satisfied {
            PrerequisiteOutcome::Satisfied
        } else {
            PrerequisiteOutcome::Unsatisfied
        };
        report(trace, prereq, outcome, Some(&result));
        if !satisfied {
            return false;
        }
    }
    true
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
    use std::sync::Arc;

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

    // ---------------------------------------------------------------------
    // Rollout stability: determinism, monotonicity, pinned buckets, and
    // evaluation while the store is being updated.
    // ---------------------------------------------------------------------

    fn rollout_flag(key: &str, percentage: u32) -> Flag {
        bool_flag(key, true, Some(percentage), vec![])
    }

    fn user(key: &str) -> UserContext {
        UserContext {
            key: key.into(),
            attributes: HashMap::new(),
        }
    }

    #[test]
    fn rollout_buckets_are_pinned_across_releases() {
        // (flag key, user key, bucket = murmur3_x86_32("{flag}:{user}") % 100).
        // A user is in the rollout iff bucket < percentage. These values are
        // what every release since bucketing was introduced has computed; if
        // this test fails, upgrading would move users in or out of rollouts.
        let pinned = [
            ("new-checkout", "user-1", 31),
            ("new-checkout", "user-2", 89),
            ("dark-mode", "alice@example.com", 52),
            ("dark-mode", "", 79),
            ("", "user-1", 17),
            ("search-v2", "8f14e45f-ceea-467f-a0e6-5e7d1c2b9a31", 72),
            ("caf\u{e9}", "\u{1F680}", 14),
        ];
        let store = FlagStore::new();
        for (flag_key, user_key, bucket) in pinned {
            let ctx = user(user_key);
            assert!(
                !evaluate(&rollout_flag(flag_key, bucket), &ctx, &store),
                "{flag_key}/{user_key} enabled at {bucket}%"
            );
            assert!(
                evaluate(&rollout_flag(flag_key, bucket + 1), &ctx, &store),
                "{flag_key}/{user_key} disabled at {}%",
                bucket + 1
            );
        }
    }

    #[test]
    fn rollout_is_deterministic_and_monotonic_in_percentage() {
        // Exhaustive over every percentage for 2 000 users: raising the
        // percentage never removes a user, lowering it never adds one, and the
        // result never depends on the store instance or on attributes the
        // flag does not target.
        let store_a = FlagStore::new();
        let store_b = FlagStore::new();
        for i in 0..2_000 {
            let key = format!("user-{i}");
            let plain = user(&key);
            let mut decorated = user(&key);
            decorated
                .attributes
                .insert("plan".into(), format!("plan-{}", i % 7));
            let mut previous = false;
            for percentage in 0..=100 {
                let flag = rollout_flag("monotonic", percentage);
                let enabled = evaluate(&flag, &plain, &store_a);
                assert_eq!(enabled, evaluate(&flag, &plain, &store_a), "repeat");
                assert_eq!(enabled, evaluate(&flag, &plain, &store_b), "store");
                assert_eq!(enabled, evaluate(&flag, &decorated, &store_a), "attrs");
                assert!(!previous || enabled, "{key} dropped at {percentage}%");
                previous = enabled;
            }
            assert!(previous, "{key} not enabled at 100%");
        }
    }

    #[test]
    fn rollout_share_tracks_the_percentage() {
        for percentage in [1u32, 10, 25, 50, 90] {
            let flag = rollout_flag("share", percentage);
            let store = FlagStore::new();
            let on = (0..20_000)
                .filter(|i| evaluate(&flag, &user(&format!("u{i}")), &store))
                .count();
            let expected = 200 * percentage as usize;
            let tolerance = expected / 10 + 60;
            assert!(
                on.abs_diff(expected) <= tolerance,
                "{percentage}%: {on} of 20000"
            );
        }
    }

    #[test]
    fn concurrent_updates_never_produce_a_mixed_result() {
        // Writers flip the flag between two complete versions while readers
        // evaluate. Each evaluation sees one version or the other (a flag is
        // swapped as one `Arc`), never a blend — e.g. B's rules with A's
        // rollout.
        let store = FlagStore::new();
        let version_a = {
            let mut f = rollout_flag("live", 30);
            f.rules = vec![string_rule("plan", Operator::Equals, &["pro"])];
            f
        };
        let version_b = rollout_flag("live", 70);
        store.upsert_flag(version_a.clone());

        let users: Vec<UserContext> = (0..200)
            .map(|i| {
                ctx_attr(
                    &format!("u{i}"),
                    "plan",
                    if i % 3 == 0 { "pro" } else { "free" },
                )
            })
            .collect();
        let empty = FlagStore::new();
        let allowed: Vec<(bool, bool)> = users
            .iter()
            .map(|u| {
                (
                    evaluate(&version_a, u, &empty),
                    evaluate(&version_b, u, &empty),
                )
            })
            .collect();

        let stop = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for i in 0..20_000 {
                    store.upsert_flag(if i % 2 == 0 {
                        version_b.clone()
                    } else {
                        version_a.clone()
                    });
                }
                stop.store(true, std::sync::atomic::Ordering::Release);
            });
            for _ in 0..4 {
                scope.spawn(|| {
                    while !stop.load(std::sync::atomic::Ordering::Acquire) {
                        for (u, (a, b)) in users.iter().zip(&allowed) {
                            let flag = store.get_flag("live").unwrap();
                            let got = evaluate(&flag, u, &store);
                            assert!(got == *a || got == *b, "{}: {got}", u.key);
                        }
                    }
                });
            }
        });
    }

    /// Builds `ladder-0 … ladder-{levels-1}`: `ladder-0` has no prerequisites,
    /// and every other level lists the level below it `fan_out` times. Every
    /// prerequisite is satisfied, so `all()` never short-circuits and naive
    /// recursion evaluates `fan_out^(levels-1)` leaves.
    fn prerequisite_ladder(store: &FlagStore, levels: usize, fan_out: usize) -> Arc<Flag> {
        store.upsert_flag(flag_with_prerequisites("ladder-0", true, vec![]));
        for level in 1..levels {
            let below = format!("ladder-{}", level - 1);
            let prerequisites = (0..fan_out)
                .map(|_| Prerequisite {
                    flag_key: below.clone(),
                    required_value: None,
                })
                .collect();
            store.upsert_flag(flag_with_prerequisites(
                &format!("ladder-{level}"),
                true,
                prerequisites,
            ));
        }
        store.get_flag(&format!("ladder-{}", levels - 1)).unwrap()
    }

    #[test]
    fn test_prerequisite_fan_out_is_bounded_and_fails_closed() {
        // 10 levels × 8 duplicate prerequisites = 8^9 ≈ 134 million leaf
        // evaluations without a budget: minutes of CPU in every SDK for one
        // `isEnabled`, from a configuration any editor can save.
        let store = FlagStore::new();
        let top = prerequisite_ladder(&store, MAX_PREREQUISITE_DEPTH, 8);
        let ctx = UserContext {
            key: "u".into(),
            attributes: HashMap::new(),
        };
        let started = std::time::Instant::now();
        assert!(!evaluate(top.as_ref(), &ctx, &store));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn test_prerequisite_diamonds_within_budget_still_evaluate() {
        // Doubling at every level over 9 levels is 2 + 4 + … + 256 = 510
        // prerequisite evaluations: inside the budget, so a shared dependency
        // reached by many paths still evaluates normally.
        let store = FlagStore::new();
        let top = prerequisite_ladder(&store, 9, 2);
        let ctx = UserContext {
            key: "u".into(),
            attributes: HashMap::new(),
        };
        assert!(evaluate(top.as_ref(), &ctx, &store));
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

    // -----------------------------------------------------------------
    // explain(): same result as evaluate_variant, plus the path taken
    // -----------------------------------------------------------------

    fn ctx(key: &str, attrs: &[(&str, &str)]) -> UserContext {
        UserContext {
            key: key.into(),
            attributes: attrs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        }
    }

    fn rule(attribute: &str, operator: Operator, values: &[&str]) -> TargetingRule {
        TargetingRule {
            attribute: attribute.into(),
            operator,
            values: values.iter().map(|v| (*v).to_owned()).collect(),
            segment_key: None,
            variant: None,
        }
    }

    #[test]
    fn explain_reports_matched_rule_index() {
        let flag = bool_flag(
            "f",
            true,
            Some(0),
            vec![
                rule("plan", Operator::Equals, &["pro"]),
                rule("country", Operator::Equals, &["PH"]),
            ],
        );
        let e = explain(&flag, &ctx("u", &[("country", "PH")]), &FlagStore::new());
        assert_eq!(e.reason, Reason::TargetingMatch);
        assert_eq!(e.rule_index, Some(1));
        assert!(e.enabled);
        let e = explain(&flag, &ctx("u", &[]), &FlagStore::new());
        assert_eq!(e.reason, Reason::RolloutExcluded);
        assert_eq!(e.rule_index, None);
        assert_eq!(e.rollout_bucket, None, "0% is decided without hashing");
    }

    #[test]
    fn explain_reports_rollout_bucket_consistently_with_the_gate() {
        let flag = bool_flag("gate", true, Some(40), vec![]);
        for i in 0..500 {
            let e = explain(&flag, &ctx(&format!("user{i}"), &[]), &FlagStore::new());
            let bucket = e.rollout_bucket.expect("partial rollout is hashed");
            assert!(bucket < 100);
            assert_eq!(e.enabled, bucket < 40);
            assert_eq!(
                e.reason,
                if e.enabled {
                    Reason::Split
                } else {
                    Reason::RolloutExcluded
                }
            );
        }
    }

    #[test]
    fn explain_reports_static_default_and_variant() {
        let store = FlagStore::new();
        let plain = bool_flag("plain", true, None, vec![]);
        assert_eq!(
            explain(&plain, &ctx("u", &[]), &store).reason,
            Reason::Static
        );
        let full = bool_flag("full", true, Some(100), vec![]);
        assert_eq!(
            explain(&full, &ctx("u", &[]), &store).reason,
            Reason::Static
        );
        let ruled = bool_flag(
            "ruled",
            true,
            None,
            vec![rule("a", Operator::Equals, &["b"])],
        );
        assert_eq!(
            explain(&ruled, &ctx("u", &[]), &store).reason,
            Reason::Default
        );

        let mut weighted = bool_flag("w", true, None, vec![]);
        weighted.flag_type = FlagType::String;
        weighted.variants = vec![
            WeightedVariant {
                weight: 1,
                value: FlagValue::Str("x".into()),
            },
            WeightedVariant {
                weight: 1,
                value: FlagValue::Str("y".into()),
            },
        ];
        let e = explain(&weighted, &ctx("u", &[]), &store);
        assert_eq!(e.reason, Reason::Split);
        let index = e.variant_index.unwrap();
        assert_eq!(e.value, weighted.variants[index].value);
    }

    #[test]
    fn explain_reports_prerequisites_and_stops_at_the_first_failure() {
        let store = FlagStore::new();
        store.upsert_flag(bool_flag("on", true, None, vec![]));
        store.upsert_flag(bool_flag("off", false, None, vec![]));
        let mut flag = bool_flag("child", true, None, vec![]);
        flag.prerequisites = vec![
            Prerequisite {
                flag_key: "on".into(),
                required_value: None,
            },
            Prerequisite {
                flag_key: "off".into(),
                required_value: None,
            },
            Prerequisite {
                flag_key: "never-checked".into(),
                required_value: None,
            },
        ];
        let e = explain(&flag, &ctx("u", &[]), &store);
        assert_eq!(e.reason, Reason::PrerequisiteFailed);
        assert!(!e.enabled);
        let outcomes: Vec<_> = e
            .prerequisites
            .iter()
            .map(|p| (p.flag_key.as_str(), p.outcome))
            .collect();
        assert_eq!(
            outcomes,
            vec![
                ("on", PrerequisiteOutcome::Satisfied),
                ("off", PrerequisiteOutcome::Unsatisfied)
            ]
        );
        assert_eq!(
            e.prerequisites[1].result,
            Some(EvalResult {
                enabled: false,
                value: FlagValue::Bool(false)
            })
        );

        flag.prerequisites = vec![Prerequisite {
            flag_key: "ghost".into(),
            required_value: None,
        }];
        let e = explain(&flag, &ctx("u", &[]), &store);
        assert_eq!(e.prerequisites[0].outcome, PrerequisiteOutcome::NotFound);
        assert_eq!(e.prerequisites[0].result, None);
    }

    #[test]
    fn explain_reports_a_cycle_as_a_failed_prerequisite() {
        let store = FlagStore::new();
        let mut a = bool_flag("a", true, None, vec![]);
        a.prerequisites = vec![Prerequisite {
            flag_key: "b".into(),
            required_value: None,
        }];
        let mut b = bool_flag("b", true, None, vec![]);
        b.prerequisites = vec![Prerequisite {
            flag_key: "a".into(),
            required_value: None,
        }];
        store.upsert_flag(a.clone());
        store.upsert_flag(b);
        let e = explain(&a, &ctx("u", &[]), &store);
        assert_eq!(e.reason, Reason::PrerequisiteFailed);
        // The direct prerequisite was evaluated (and failed further down).
        assert_eq!(e.prerequisites[0].outcome, PrerequisiteOutcome::Unsatisfied);
    }

    #[test]
    fn explanation_serializes_with_the_documented_wire_names() {
        let flag = bool_flag("f", true, Some(50), vec![]);
        let e = explain(&flag, &ctx("u", &[]), &FlagStore::new());
        let v = serde_json::to_value(&e).unwrap();
        assert!(v["reason"] == "SPLIT" || v["reason"] == "ROLLOUT_EXCLUDED");
        assert!(v["rollout_bucket"].is_u64());
        assert!(v.get("rule_index").is_none(), "absent fields are omitted");
        assert!(v.get("prerequisites").is_none());
    }

    /// A deterministic stream of pseudo-random numbers (no new dependency).
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self, bound: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % bound
        }
    }

    #[test]
    fn explain_agrees_with_evaluate_variant_on_random_configurations() {
        let ops = [
            Operator::Equals,
            Operator::NotEquals,
            Operator::Contains,
            Operator::StartsWith,
            Operator::EndsWith,
            Operator::GreaterThan,
            Operator::GreaterThanOrEqual,
            Operator::LessThan,
            Operator::LessThanOrEqual,
        ];
        let words = ["1", "5", "10", "pro", "free", "ph", "x"];
        let mut rng = Lcg(0x5eed);
        for round in 0..300 {
            let store = FlagStore::new();
            let count = 6;
            for i in 0..count {
                let mut f = bool_flag(&format!("f{i}"), rng.next(5) != 0, None, vec![]);
                f.rollout_percentage = match rng.next(4) {
                    0 => None,
                    1 => Some(0),
                    2 => Some(100),
                    _ => Some(u32::try_from(rng.next(100)).unwrap()),
                };
                for _ in 0..rng.next(3) {
                    let mut r = rule(
                        ["a", "b", "c"][usize::try_from(rng.next(3)).unwrap()],
                        ops[usize::try_from(rng.next(9)).unwrap()].clone(),
                        &[words[usize::try_from(rng.next(7)).unwrap()]],
                    );
                    if rng.next(2) == 0 {
                        r.variant = Some(FlagValue::Str("rule".into()));
                    }
                    f.rules.push(r);
                }
                if rng.next(3) == 0 {
                    f.flag_type = FlagType::String;
                    f.variants = (0..rng.next(4))
                        .map(|w| WeightedVariant {
                            weight: u32::try_from(rng.next(5)).unwrap(),
                            value: FlagValue::Str(format!("v{w}")),
                        })
                        .collect();
                }
                for _ in 0..rng.next(3) {
                    f.prerequisites.push(Prerequisite {
                        flag_key: format!("f{}", rng.next(count + 1)),
                        required_value: None,
                    });
                }
                store.upsert_flag(f);
            }
            for u in 0..20 {
                let c = ctx(
                    &format!("r{round}u{u}"),
                    &[
                        ("a", words[usize::try_from(rng.next(7)).unwrap()]),
                        ("b", words[usize::try_from(rng.next(7)).unwrap()]),
                    ],
                );
                for f in store.list_flags() {
                    let e = explain(&f, &c, &store);
                    assert_eq!(e.result(), evaluate_variant(&f, &c, &store));
                    assert_eq!(e.rule_index.is_some(), e.reason == Reason::TargetingMatch);
                    if let Some(i) = e.rule_index {
                        assert!(rule_matches(&f.rules[i], &c));
                    }
                }
            }
        }
    }
}
