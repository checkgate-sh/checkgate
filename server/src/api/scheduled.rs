use crate::auth::{AuthContext, get_session_claims};
use crate::state::AppState;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use tracing::{error, info};

use super::flags::check_env_access;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ScheduledChange {
    pub id: String,
    pub environment_id: String,
    pub flag_key: String,
    pub scheduled_at: String,
    pub patch: Value,
    pub executed_at: Option<String>,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub created_at: String,
}

#[derive(Deserialize)]
struct CreateBody {
    /// RFC-3339 / ISO-8601 timestamp at which the patch should be applied.
    scheduled_at: String,
    /// JSON merge-patch to apply to the flag at `scheduled_at`.
    patch: Value,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn read_router() -> Router<AppState> {
    Router::new()
        .route(
            "/environments/{env_id}/scheduled-changes",
            get(list_scheduled),
        )
        .route(
            "/environments/{env_id}/flags/{key}/scheduled-changes",
            get(list_scheduled_for_flag),
        )
}

pub fn write_router() -> Router<AppState> {
    Router::new()
        .route(
            "/environments/{env_id}/flags/{key}/scheduled-changes",
            post(create_scheduled),
        )
        .route(
            "/environments/{env_id}/scheduled-changes/{id}",
            delete(delete_scheduled),
        )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_scheduled(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(env_id): Path<String>,
) -> Result<Json<Vec<ScheduledChange>>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    let rows = sqlx::query(
        "SELECT id::text, environment_id::text, flag_key, \
         scheduled_at::text, patch, executed_at::text, created_at::text, attempts, last_error \
         FROM scheduled_changes WHERE environment_id = $1::uuid \
         ORDER BY scheduled_at ASC",
    )
    .bind(&env_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to list scheduled changes");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(rows_to_changes(&rows)))
}

async fn list_scheduled_for_flag(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, key)): Path<(String, String)>,
) -> Result<Json<Vec<ScheduledChange>>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    let rows = sqlx::query(
        "SELECT id::text, environment_id::text, flag_key, \
         scheduled_at::text, patch, executed_at::text, created_at::text, attempts, last_error \
         FROM scheduled_changes \
         WHERE environment_id = $1::uuid AND flag_key = $2 \
         ORDER BY scheduled_at ASC",
    )
    .bind(&env_id)
    .bind(&key)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to list scheduled changes for flag");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(rows_to_changes(&rows)))
}

async fn create_scheduled(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, key)): Path<(String, String)>,
    Json(body): Json<CreateBody>,
) -> Result<Json<ScheduledChange>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if super::flags::lock_environment(&mut tx, &env_id).await? {
        return Err(StatusCode::CONFLICT);
    }
    super::flags::merge_and_validate(&mut *tx, &env_id, &key, body.patch.clone()).await?;
    let scheduled_at = time::OffsetDateTime::parse(
        &body.scheduled_at,
        &time::format_description::well_known::Rfc3339,
    )
    .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    if scheduled_at.unix_timestamp()
        < time::OffsetDateTime::now_utc()
            .unix_timestamp()
            .saturating_sub(30)
    {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let row = sqlx::query(
        "INSERT INTO scheduled_changes (environment_id, flag_key, scheduled_at, patch) \
         VALUES ($1::uuid, $2, $3, $4) \
         RETURNING id::text, environment_id::text, flag_key, \
                   scheduled_at::text, patch, executed_at::text, created_at::text, attempts, last_error",
    )
    .bind(&env_id)
    .bind(&key)
    .bind(scheduled_at)
    .bind(&body.patch)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to create scheduled change");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let actor = get_session_claims(&jar).map(|c| c.email);
    info!(
        env_id = %env_id,
        flag_key = %key,
        actor = ?actor,
        scheduled_at = %body.scheduled_at,
        "Scheduled change created"
    );

    Ok(Json(row_to_change(&row)))
}

async fn delete_scheduled(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, id)): Path<(String, String)>,
) -> Result<StatusCode, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    let result = sqlx::query(
        "DELETE FROM scheduled_changes \
         WHERE id = $1::uuid AND environment_id = $2::uuid AND executed_at IS NULL",
    )
    .bind(&id)
    .bind(&env_id)
    .execute(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to delete scheduled change");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if result.rows_affected() == 0 {
        // Either not found or already executed.
        return Err(StatusCode::NOT_FOUND);
    }

    info!(env_id = %env_id, scheduled_change_id = %id, "Scheduled change deleted");
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn rows_to_changes(rows: &[sqlx::postgres::PgRow]) -> Vec<ScheduledChange> {
    rows.iter().map(row_to_change).collect()
}

fn row_to_change(r: &sqlx::postgres::PgRow) -> ScheduledChange {
    ScheduledChange {
        id: r.get("id"),
        environment_id: r.get("environment_id"),
        flag_key: r.get("flag_key"),
        scheduled_at: r.get("scheduled_at"),
        patch: r.get("patch"),
        executed_at: r.get("executed_at"),
        attempts: r.get("attempts"),
        last_error: r.get("last_error"),
        created_at: r.get("created_at"),
    }
}
