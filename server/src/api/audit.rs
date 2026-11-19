use crate::auth::AuthContext;
use crate::state::AppState;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Row};
use tracing::error;

use super::flags::check_env_access;

#[derive(Debug, Serialize)]
pub struct AuditEntry {
    pub id: i64,
    pub environment_id: String,
    pub flag_key: String,
    pub actor_email: Option<String>,
    pub action: String,
    pub before_data: Option<Value>,
    pub after_data: Option<Value>,
    pub metadata: Option<Value>,
    pub created_at: String,
}

#[derive(Deserialize)]
struct AuditQuery {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    flag_key: Option<String>,
}

fn default_limit() -> i64 {
    50
}

pub fn read_router() -> Router<AppState> {
    Router::new().route("/environments/{env_id}/audit", get(list_audit))
}

async fn list_audit(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(env_id): Path<String>,
    Query(q): Query<AuditQuery>,
) -> Result<Json<Vec<AuditEntry>>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;
    // Clamped like the flag list: an unbounded `limit` returned the whole
    // audit history (with before/after JSON) in one response, and a negative
    // value reached PostgreSQL as an invalid LIMIT/OFFSET (500).
    let limit = q.limit.clamp(0, super::flags::MAX_PAGE_LIMIT);
    let offset = q.offset.clamp(0, super::flags::MAX_PAGE_OFFSET);

    let rows = if let Some(ref flag_key) = q.flag_key {
        sqlx::query(
            "SELECT id, environment_id::text, flag_key, actor_email, action, \
             before_data, after_data, metadata, created_at::text \
             FROM flag_audit_log \
             WHERE environment_id = $1::uuid AND flag_key = $2 \
             ORDER BY created_at DESC LIMIT $3 OFFSET $4",
        )
        .bind(&env_id)
        .bind(flag_key)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query(
            "SELECT id, environment_id::text, flag_key, actor_email, action, \
             before_data, after_data, metadata, created_at::text \
             FROM flag_audit_log \
             WHERE environment_id = $1::uuid \
             ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(&env_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
    }
    .map_err(|e| {
        error!(error = %e, "Failed to fetch audit log");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let entries = rows
        .iter()
        .map(|r| AuditEntry {
            id: r.get("id"),
            environment_id: r.get("environment_id"),
            flag_key: r.get("flag_key"),
            actor_email: r.get("actor_email"),
            action: r.get("action"),
            before_data: r.get("before_data"),
            after_data: r.get("after_data"),
            metadata: r.get("metadata"),
            created_at: r.get("created_at"),
        })
        .collect();

    Ok(Json(entries))
}

/// Appends a row to `flag_audit_log`. Errors are logged but never propagated
/// — the audit write must not cause the primary operation to fail.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn log_audit_event(
    db: &PgPool,
    env_id: &str,
    flag_key: &str,
    actor_email: Option<&str>,
    action: &str,
    before_data: Option<&Value>,
    after_data: Option<&Value>,
    metadata: Option<&Value>,
) {
    if let Err(e) = sqlx::query(
        "INSERT INTO flag_audit_log \
         (environment_id, flag_key, actor_email, action, before_data, after_data, metadata) \
         VALUES ($1::uuid, $2, $3, $4, $5, $6, $7)",
    )
    .bind(env_id)
    .bind(flag_key)
    .bind(actor_email)
    .bind(action)
    .bind(before_data)
    .bind(after_data)
    .bind(metadata)
    .execute(db)
    .await
    {
        error!(error = %e, "Failed to write audit log entry");
    }
}
