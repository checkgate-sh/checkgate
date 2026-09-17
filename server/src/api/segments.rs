use crate::auth::AuthContext;
use crate::state::AppState;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use checkgate_core::evaluator::{Flag, TargetingRule};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use tracing::{error, info, warn};

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
    description: Option<String>,
    rules: Option<Vec<TargetingRule>>,
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
    db: &PgPool,
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

    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

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
         description = COALESCE($2, description), \
         rules       = COALESCE($3, rules) \
         WHERE environment_id = $4::uuid AND key = $5 \
         RETURNING id::text, environment_id::text, name, key, description, rules, created_at::text",
    )
    .bind(&body.name)
    .bind(&body.description)
    .bind(&rules_val)
    .bind(&env_id)
    .bind(&key)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to update segment");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

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
    rebroadcast_referencing_flags(&state, &env_id, &key).await;

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

    // Commit deletion before re-expanding referencing flags without these rules.
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

    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    rebroadcast_referencing_flags(&state, &env_id, &key).await;

    info!(env_id = %env_id, key = %key, "Segment deleted");
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Find all flags in `env_id` that reference `seg_key` in any targeting rule,
/// re-expand them with the current segment state, and re-publish via Redis.
async fn rebroadcast_referencing_flags(state: &AppState, env_id: &str, seg_key: &str) {
    let segment_ref = json!([{"segment_key": seg_key}]);

    let rows = match sqlx::query(
        "SELECT key, data FROM flags \
         WHERE environment_id = $1::uuid \
           AND data->'rules' @> $2::jsonb",
    )
    .bind(env_id)
    .bind(&segment_ref)
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "Failed to query flags referencing segment");
            return;
        }
    };

    if rows.is_empty() {
        return;
    }

    let segment_map = match load_env_segments(env_id, &state.db).await {
        Ok(m) => m,
        Err(e) => {
            error!(error = %e, "Failed to load segment map for rebroadcast");
            return;
        }
    };

    let mut conn = state.redis_conn.clone();

    for row in rows {
        let flag_key: String = row.get("key");
        let data_val: Value = match row.try_get("data") {
            Ok(v) => v,
            Err(_) => continue,
        };
        let raw_flag: Flag = match serde_json::from_value(data_val) {
            Ok(f) => f,
            Err(_) => continue,
        };

        let expanded = expand_flag_with_segments(raw_flag, &segment_map);
        let msg = json!({"type": "UPSERT", "env_id": env_id, "flag": expanded}).to_string();

        // Also update FlagStore on this instance.
        state.store.upsert_flag(expanded);

        if let Err(e) = conn.publish::<_, _, ()>("checkgate_updates", &msg).await {
            warn!(
                error = %e,
                flag_key = %flag_key,
                "Redis publish failed during segment rebroadcast"
            );
        }
    }
}
