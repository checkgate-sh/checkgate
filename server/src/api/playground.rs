//! Evaluation playground: evaluate one flag for a hypothetical user, and
//! explain the result, without touching anything.
//!
//! `POST /api/environments/{env_id}/flags/{key}/evaluate`
//!
//! The flags are read with the same statement the SDK snapshot uses and
//! expanded with the same segment code, then evaluated by
//! `checkgate_core::evaluator::explain` — the code every SDK runs. So the
//! answer is the one an SDK holding this revision gives for the same user.
//!
//! Nothing is written: no audit entry, no impression, no stored context. The
//! request body (user key and attributes) is neither logged nor kept. Access
//! is the environment's read access (project members and workspace admins;
//! `read_only` tokens included, SDK keys excluded by the route allowlist).

use crate::api::segments::{RuleOrigin, expand_flag_with_origins};
use crate::auth::AuthContext;
use crate::state::AppState;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use checkgate_core::evaluator::{
    Explanation, Flag, FlagType, FlagValue, TargetingRule, UserContext, explain,
};
use checkgate_core::store::FlagStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};
use tracing::{error, instrument};

use super::flags::check_env_access;

/// Longest accepted targeting key, in bytes.
pub(crate) const MAX_TARGETING_KEY_BYTES: usize = 512;
/// Most attributes one request may carry.
pub(crate) const MAX_ATTRIBUTES: usize = 100;
/// Longest attribute name, in bytes.
pub(crate) const MAX_ATTRIBUTE_NAME_BYTES: usize = 128;
/// Longest attribute value (after conversion to a string), in bytes.
pub(crate) const MAX_ATTRIBUTE_VALUE_BYTES: usize = 1024;

pub fn read_router() -> Router<AppState> {
    Router::new().route(
        "/environments/{env_id}/flags/{key}/evaluate",
        post(evaluate_flag),
    )
}

#[derive(Debug, Deserialize)]
pub struct EvaluatePath {
    env_id: String,
    key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluateRequest {
    targeting_key: String,
    #[serde(default)]
    attributes: serde_json::Map<String, Value>,
}

/// The rule that matched, as the SDK evaluated it and as it is stored.
#[derive(Debug, Serialize)]
struct MatchedRule {
    rule: TargetingRule,
    #[serde(flatten)]
    origin: RuleOrigin,
}

#[derive(Debug, Serialize)]
struct RolloutInfo {
    percentage: u32,
    bucket: u32,
}

#[derive(Debug, Serialize)]
struct VariantInfo {
    index: usize,
    weight: u32,
    total_weight: u64,
    value: FlagValue,
}

#[derive(Debug, Serialize)]
struct PrerequisiteInfo {
    #[serde(flatten)]
    check: checkgate_core::evaluator::PrerequisiteCheck,
    /// Revision of the prerequisite flag evaluated, when it exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<i64>,
}

#[derive(Debug, Serialize)]
struct EvaluateResponse {
    environment_id: String,
    flag_key: String,
    flag_type: FlagType,
    /// Revision of the flag configuration evaluated (the number SDKs track).
    revision: i64,
    enabled: bool,
    value: FlagValue,
    reason: checkgate_core::evaluator::Reason,
    #[serde(skip_serializing_if = "Option::is_none")]
    matched_rule: Option<MatchedRule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rollout: Option<RolloutInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    variant: Option<VariantInfo>,
    prerequisites: Vec<PrerequisiteInfo>,
    /// Attributes the flag's rules (and its prerequisites' rules) read.
    referenced_attributes: Vec<String>,
    /// Referenced attributes the request did not provide.
    missing_attributes: Vec<String>,
}

/// A request the playground refuses, with a message the dashboard shows.
fn invalid(message: impl Into<String>) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": message.into() })),
    )
        .into_response()
}

/// Converts and bounds the attributes. Checkgate attributes are strings;
/// numbers and booleans are accepted as their string form (numeric operators
/// parse them back), anything else is refused rather than silently dropped,
/// so what is evaluated is exactly what was entered.
pub(crate) fn validate_attributes(
    attributes: serde_json::Map<String, Value>,
) -> Result<HashMap<String, String>, String> {
    if attributes.len() > MAX_ATTRIBUTES {
        return Err(format!("At most {MAX_ATTRIBUTES} attributes."));
    }
    let mut out = HashMap::with_capacity(attributes.len());
    for (name, value) in attributes {
        if name.is_empty() || name.len() > MAX_ATTRIBUTE_NAME_BYTES {
            return Err(format!(
                "Attribute names must be 1 to {MAX_ATTRIBUTE_NAME_BYTES} bytes."
            ));
        }
        let text = match value {
            Value::String(s) => s,
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null | Value::Array(_) | Value::Object(_) => {
                return Err(format!(
                    "Attribute {name:?} must be a string, number or boolean."
                ));
            }
        };
        if text.len() > MAX_ATTRIBUTE_VALUE_BYTES {
            return Err(format!(
                "Attribute {name:?} is longer than {MAX_ATTRIBUTE_VALUE_BYTES} bytes."
            ));
        }
        out.insert(name, text);
    }
    Ok(out)
}

/// Attribute names read by `flag`'s rules and, transitively, by its
/// prerequisites' rules (each flag visited once).
fn referenced_attributes(flag: &Flag, store: &FlagStore) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut seen = HashSet::from([flag.key.clone()]);
    let mut pending = vec![std::sync::Arc::new(flag.clone())];
    while let Some(current) = pending.pop() {
        for rule in &current.rules {
            if !rule.attribute.is_empty() {
                names.insert(rule.attribute.clone());
            }
        }
        for prerequisite in &current.prerequisites {
            if seen.insert(prerequisite.flag_key.clone())
                && let Some(found) = store.get_flag(&prerequisite.flag_key)
            {
                pending.push(found);
            }
        }
    }
    names
}

#[instrument(skip(state, jar, body), fields(flag_key = %path.key))]
async fn evaluate_flag(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(path): Path<EvaluatePath>,
    body: Result<Json<EvaluateRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, StatusCode> {
    check_env_access(&state.db, &jar, &path.env_id).await?;
    if !super::flags::is_valid_flag_key(&path.key) {
        return Err(StatusCode::NOT_FOUND);
    }
    let request = match body {
        Ok(Json(request)) => request,
        // An over-limit body keeps its own status (413), as on every route.
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        Err(_) => {
            return Ok(invalid(
                "Expected {\"targeting_key\": string, \"attributes\": object}.",
            ));
        }
    };
    if request.targeting_key.len() > MAX_TARGETING_KEY_BYTES {
        return Ok(invalid(format!(
            "The targeting key is longer than {MAX_TARGETING_KEY_BYTES} bytes."
        )));
    }
    let attributes = match validate_attributes(request.attributes) {
        Ok(a) => a,
        Err(message) => return Ok(invalid(message)),
    };

    let raw = crate::stream::read_raw_snapshot(&state, &path.env_id, true)
        .await
        .map_err(|e| {
            error!(error = %e, "playground: failed to read the environment's flags");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let store = FlagStore::new();
    let mut target = None;
    let mut revisions = HashMap::with_capacity(raw.flags.len());
    for (index, flag) in raw.flags.into_iter().enumerate() {
        let revision = raw.revisions.get(index).copied().unwrap_or_default();
        revisions.insert(flag.key.clone(), revision);
        let (expanded, origins) = expand_flag_with_origins(flag, &raw.segment_map);
        if expanded.key == path.key {
            target = Some((expanded.clone(), origins, revision));
        }
        store.upsert_flag(expanded);
    }
    let Some((flag, origins, revision)) = target else {
        return Err(StatusCode::NOT_FOUND);
    };

    let context = UserContext {
        key: request.targeting_key,
        attributes,
    };
    let explanation: Explanation = explain(&flag, &context, &store);

    let matched_rule = explanation.rule_index.and_then(|i| {
        Some(MatchedRule {
            rule: flag.rules.get(i)?.clone(),
            origin: origins.get(i)?.clone(),
        })
    });
    let rollout = explanation
        .rollout_bucket
        .zip(flag.rollout_percentage)
        .map(|(bucket, percentage)| RolloutInfo { percentage, bucket });
    let variant = explanation.variant_index.and_then(|index| {
        let chosen = flag.variants.get(index)?;
        Some(VariantInfo {
            index,
            weight: chosen.weight,
            total_weight: flag
                .variants
                .iter()
                .map(|v| u64::from(v.weight))
                .fold(0, u64::saturating_add),
            value: chosen.value.clone(),
        })
    });
    let referenced = referenced_attributes(&flag, &store);
    let missing = referenced
        .iter()
        .filter(|name| !context.attributes.contains_key(*name))
        .cloned()
        .collect();
    let prerequisites = explanation
        .prerequisites
        .iter()
        .map(|check| PrerequisiteInfo {
            revision: revisions.get(&check.flag_key).copied(),
            check: check.clone(),
        })
        .collect();

    Ok(Json(EvaluateResponse {
        environment_id: path.env_id,
        flag_key: flag.key.clone(),
        flag_type: flag.flag_type.clone(),
        revision,
        enabled: explanation.enabled,
        value: explanation.value,
        reason: explanation.reason,
        matched_rule,
        rollout,
        variant,
        prerequisites,
        referenced_attributes: referenced.into_iter().collect(),
        missing_attributes: missing,
    })
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(v: Value) -> serde_json::Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn scalar_attributes_become_strings() {
        let a = validate_attributes(map(
            json!({"plan": "pro", "age": 30, "beta": true, "x": 1.5}),
        ))
        .unwrap();
        assert_eq!(a["plan"], "pro");
        assert_eq!(a["age"], "30");
        assert_eq!(a["beta"], "true");
        assert_eq!(a["x"], "1.5");
    }

    #[test]
    fn non_scalar_and_oversized_attributes_are_refused() {
        for bad in [
            json!({"o": {"a": 1}}),
            json!({"l": [1]}),
            json!({"n": null}),
            json!({"": "x"}),
            json!({"k": "v".repeat(MAX_ATTRIBUTE_VALUE_BYTES + 1)}),
        ] {
            assert!(validate_attributes(map(bad.clone())).is_err(), "{bad}");
        }
        let long_name = "n".repeat(MAX_ATTRIBUTE_NAME_BYTES + 1);
        assert!(validate_attributes(map(json!({ long_name: "v" }))).is_err());
        let many: serde_json::Map<String, Value> = (0..=MAX_ATTRIBUTES)
            .map(|i| (format!("a{i}"), json!("v")))
            .collect();
        assert!(validate_attributes(many).is_err());
    }
}
