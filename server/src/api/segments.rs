use crate::auth::AuthContext;
use crate::state::AppState;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use checkgate_core::evaluator::{Flag, TargetingRule};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use tracing::{error, info};

use super::flags::check_env_access;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub id: String,
    pub environment_id: String,
    pub name: String,
    pub key: String,
    pub description: Option<String>,
    pub rules: Vec<TargetingRule>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
struct CreateSegmentBody {
    name: String,
    key: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    rules: Vec<TargetingRule>,
}

#[derive(Debug, Deserialize)]
struct PatchSegmentBody {
    name: Option<String>,
    /// `None`: left out, keep the stored value. `Some(None)`: `null`, clear it.
    #[serde(default, deserialize_with = "present_or_null")]
    description: Option<Option<String>>,
    rules: Option<Vec<TargetingRule>>,
}

/// Deserializes a field that was present in the body (possibly as `null`), so
/// a PATCH can tell "clear this" (`null`) apart from "leave it" (omitted, via
/// `#[serde(default)]`).
fn present_or_null<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

// ---------------------------------------------------------------------------
// Segment key validation — same constraints as flag keys
// ---------------------------------------------------------------------------

fn is_valid_segment_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 100
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

// ---------------------------------------------------------------------------
// Segment expansion helpers — used by flags.rs and main.rs
// ---------------------------------------------------------------------------

/// Load all segments for an environment into a HashMap keyed by segment key.
pub(crate) async fn load_env_segments(
    env_id: &str,
    db: impl sqlx::PgExecutor<'_>,
) -> Result<HashMap<String, Vec<TargetingRule>>, sqlx::Error> {
    let rows =
        sqlx::query("SELECT key, rules FROM segments WHERE environment_id = $1::uuid ORDER BY key")
            .bind(env_id)
            .fetch_all(db)
            .await?;

    let mut map = HashMap::new();
    for row in rows {
        let key: String = row.get("key");
        if let Ok(rules_val) = row.try_get::<Value, _>("rules")
            && let Ok(rules) = serde_json::from_value::<Vec<TargetingRule>>(rules_val)
        {
            map.insert(key, rules);
        }
    }
    Ok(map)
}

/// Load all segments across all environments, keyed by `(env_id, segment_key)`.
/// Used at startup to expand flags during FlagStore bootstrap.
pub(crate) async fn load_all_segments(
    db: &PgPool,
) -> Result<HashMap<(String, String), Vec<TargetingRule>>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT environment_id::text, key, rules FROM segments ORDER BY environment_id, key",
    )
    .fetch_all(db)
    .await?;

    let mut map = HashMap::new();
    for row in rows {
        let env_id: String = row.get("environment_id");
        let key: String = row.get("key");
        if let Ok(rules_val) = row.try_get::<Value, _>("rules")
            && let Ok(rules) = serde_json::from_value::<Vec<TargetingRule>>(rules_val)
        {
            map.insert((env_id, key), rules);
        }
    }
    Ok(map)
}

/// Expand any `segment_key` references in a flag's rules to their concrete rules.
/// Uses the preloaded `segment_map` — no DB calls. Unexpanded segment keys that are
/// not found in the map are silently dropped (safe: flag falls through to default).
pub(crate) fn expand_flag_with_segments(
    flag: Flag,
    segment_map: &HashMap<String, Vec<TargetingRule>>,
) -> Flag {
    if !flag.rules.iter().any(|r| r.segment_key.is_some()) {
        return flag;
    }

    let mut expanded_rules = Vec::new();
    for rule in flag.rules {
        if let Some(ref seg_key) = rule.segment_key {
            if let Some(seg_rules) = segment_map.get(seg_key) {
                // Propagate the segment rule's variant to segment rules that don't
                // specify their own variant.
                let expanded = seg_rules.iter().map(|sr| {
                    if sr.variant.is_none() && rule.variant.is_some() {
                        TargetingRule {
                            variant: rule.variant.clone(),
                            ..sr.clone()
                        }
                    } else {
                        sr.clone()
                    }
                });
                expanded_rules.extend(expanded);
            }
            // segment_key not found → drop the rule
        } else {
            expanded_rules.push(rule);
        }
    }

    Flag {
        rules: expanded_rules,
        ..flag
    }
}

// ---------------------------------------------------------------------------
// Routers
// ---------------------------------------------------------------------------

pub fn read_router() -> Router<AppState> {
    Router::new()
        .route("/environments/{env_id}/segments", get(list_segments))
        .route("/environments/{env_id}/segments/{key}", get(get_segment))
}

pub fn write_router() -> Router<AppState> {
    Router::new()
        .route("/environments/{env_id}/segments", post(create_segment))
        .route(
            "/environments/{env_id}/segments/{key}",
            axum::routing::patch(patch_segment).delete(delete_segment),
        )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_segments(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(env_id): Path<String>,
) -> Result<Json<Vec<Segment>>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    let rows = sqlx::query(
        "SELECT id::text, environment_id::text, name, key, description, rules, created_at::text \
         FROM segments WHERE environment_id = $1::uuid ORDER BY name ASC",
    )
    .bind(&env_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to list segments");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let segments = rows
        .iter()
        .filter_map(|r| {
            let rules_val: Value = r.try_get("rules").ok()?;
            let rules = serde_json::from_value(rules_val).ok()?;
            Some(Segment {
                id: r.get("id"),
                environment_id: r.get("environment_id"),
                name: r.get("name"),
                key: r.get("key"),
                description: r.get("description"),
                rules,
                created_at: r.get("created_at"),
            })
        })
        .collect();

    Ok(Json(segments))
}

async fn get_segment(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, key)): Path<(String, String)>,
) -> Result<Json<Segment>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    let row = sqlx::query(
        "SELECT id::text, environment_id::text, name, key, description, rules, created_at::text \
         FROM segments WHERE environment_id = $1::uuid AND key = $2",
    )
    .bind(&env_id)
    .bind(&key)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error fetching segment");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

    let rules_val: Value = row
        .try_get("rules")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rules = serde_json::from_value(rules_val).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(Segment {
        id: row.get("id"),
        environment_id: row.get("environment_id"),
        name: row.get("name"),
        key: row.get("key"),
        description: row.get("description"),
        rules,
        created_at: row.get("created_at"),
    }))
}

async fn create_segment(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(env_id): Path<String>,
    Json(body): Json<CreateSegmentBody>,
) -> Result<Json<Segment>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;
    super::flags::check_env_write_access(&state.db, &jar, &env_id).await?;
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if super::flags::lock_environment(&mut tx, &env_id).await? {
        return Err(StatusCode::CONFLICT);
    }

    if !is_valid_segment_key(&body.key) {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let rules_val = serde_json::to_value(&body.rules).map_err(|e| {
        error!(error = %e, "Failed to serialize segment rules");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let row = sqlx::query(
        "INSERT INTO segments (environment_id, name, key, description, rules) \
         VALUES ($1::uuid, $2, $3, $4, $5) \
         RETURNING id::text, environment_id::text, name, key, description, rules, created_at::text",
    )
    .bind(&env_id)
    .bind(&body.name)
    .bind(&body.key)
    .bind(&body.description)
    .bind(&rules_val)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to insert segment");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let rebroadcast = rerevision_referencing_flags(&mut tx, &env_id, &body.key).await?;

    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    publish_rebroadcast(&state, rebroadcast).await;

    let rules_val: Value = row
        .try_get("rules")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rules = serde_json::from_value(rules_val).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let seg = Segment {
        id: row.get("id"),
        environment_id: row.get("environment_id"),
        name: row.get("name"),
        key: row.get("key"),
        description: row.get("description"),
        rules,
        created_at: row.get("created_at"),
    };

    info!(env_id = %env_id, key = %seg.key, "Segment created");
    Ok(Json(seg))
}

async fn patch_segment(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, key)): Path<(String, String)>,
    Json(body): Json<PatchSegmentBody>,
) -> Result<Json<Segment>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;
    super::flags::check_env_write_access(&state.db, &jar, &env_id).await?;
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if super::flags::lock_environment(&mut tx, &env_id).await? {
        return Err(StatusCode::CONFLICT);
    }

    let rules_val = body
        .rules
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| {
            error!(error = %e, "Failed to serialize rules");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let row = sqlx::query(
        "UPDATE segments SET \
         name        = COALESCE($1, name), \
         description = CASE WHEN $6 THEN $2 ELSE description END, \
         rules       = COALESCE($3, rules) \
         WHERE environment_id = $4::uuid AND key = $5 \
         RETURNING id::text, environment_id::text, name, key, description, rules, created_at::text",
    )
    .bind(&body.name)
    .bind(body.description.clone().flatten())
    .bind(&rules_val)
    .bind(&env_id)
    .bind(&key)
    .bind(body.description.is_some())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to update segment");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;
    let rebroadcast = rerevision_referencing_flags(&mut tx, &env_id, &key).await?;

    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let rules_val: Value = row
        .try_get("rules")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rules: Vec<TargetingRule> =
        serde_json::from_value(rules_val).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let seg = Segment {
        id: row.get("id"),
        environment_id: row.get("environment_id"),
        name: row.get("name"),
        key: row.get("key"),
        description: row.get("description"),
        rules,
        created_at: row.get("created_at"),
    };

    info!(env_id = %env_id, key = %key, "Segment updated — re-broadcasting referencing flags");
    publish_rebroadcast(&state, rebroadcast).await;

    Ok(Json(seg))
}

async fn delete_segment(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, key)): Path<(String, String)>,
) -> Result<StatusCode, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;
    super::flags::check_env_write_access(&state.db, &jar, &env_id).await?;
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if super::flags::lock_environment(&mut tx, &env_id).await? {
        return Err(StatusCode::CONFLICT);
    }

    let result = sqlx::query("DELETE FROM segments WHERE environment_id = $1::uuid AND key = $2")
        .bind(&env_id)
        .bind(&key)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to delete segment");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if result.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    let rebroadcast = rerevision_referencing_flags(&mut tx, &env_id, &key).await?;

    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    publish_rebroadcast(&state, rebroadcast).await;

    info!(env_id = %env_id, key = %key, "Segment deleted");
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A flag re-expanded after a segment change, with its new revision.
struct Rebroadcast {
    env_id: String,
    flag: Flag,
    revision: i64,
}

/// Gives every flag in `env_id` that references `seg_key` a new revision and
/// re-expands it against the segments as written in `tx`. Must run inside the
/// segment write's transaction: the expanded payload changes even though the
/// flag row doesn't, so without a new revision SDKs could not tell an old
/// expansion from the new one, and the row locks order concurrent edits.
async fn rerevision_referencing_flags(
    tx: &mut sqlx::PgTransaction<'_>,
    env_id: &str,
    seg_key: &str,
) -> Result<Vec<Rebroadcast>, StatusCode> {
    let segment_ref = json!([{"segment_key": seg_key}]);
    // The flags_bump_revision trigger assigns the new revision.
    let rows = sqlx::query(
        "UPDATE flags SET revision = revision \
         WHERE environment_id = $1::uuid AND data->'rules' @> $2::jsonb \
         RETURNING data, revision",
    )
    .bind(env_id)
    .bind(&segment_ref)
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to re-revision flags referencing segment");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let segment_map = load_env_segments(env_id, &mut **tx).await.map_err(|e| {
        error!(error = %e, "Failed to load segment map for rebroadcast");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let Ok(data) = row.try_get::<Value, _>("data") else {
            continue;
        };
        let Ok(flag) = serde_json::from_value::<Flag>(data) else {
            continue;
        };
        out.push(Rebroadcast {
            env_id: env_id.to_string(),
            flag: expand_flag_with_segments(flag, &segment_map),
            revision: row.get("revision"),
        });
    }
    Ok(out)
}

/// Publishes flags re-expanded by `rerevision_referencing_flags`, once the
/// segment change has committed.
async fn publish_rebroadcast(state: &AppState, flags: Vec<Rebroadcast>) {
    for item in flags {
        let msg = super::flags::upsert_message(&item.env_id, &item.flag, item.revision);
        // Also update FlagStore on this instance.
        state.store.upsert_flag(item.flag);
        super::flags::publish_update(state, &msg, "segment_rebroadcast").await;
    }
}
