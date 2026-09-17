//! Approval workflow for flag mutations. When an environment has
//! `require_approval` set, `PATCH /environments/{env_id}/flags/{key}` no
//! longer applies immediately — it's captured here as a pending change
//! request that a *different* editor/admin must approve before it takes
//! effect. See `super::flags::patch_flag` for the queueing side.

use crate::auth::{AuthContext, get_session_claims};
use crate::state::AppState;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tracing::{error, info, warn};

fn format_rfc3339(dt: time::OffsetDateTime) -> String {
    dt.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| dt.to_string())
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ChangeRequestInfo {
    pub id: i64,
    pub environment_id: String,
    pub flag_key: String,
    pub patch: serde_json::Value,
    pub requested_by: String,
    pub status: String,
    pub reviewed_by: Option<String>,
    pub reason: Option<String>,
    pub created_at: String,
    pub reviewed_at: Option<String>,
}

fn row_to_info(row: &sqlx::postgres::PgRow) -> ChangeRequestInfo {
    ChangeRequestInfo {
        id: row.get("id"),
        environment_id: row.get("environment_id"),
        flag_key: row.get("flag_key"),
        patch: row.get("patch"),
        requested_by: row.get("requested_by"),
        status: row.get("status"),
        reviewed_by: row.get("reviewed_by"),
        reason: row.get("reason"),
        created_at: format_rfc3339(row.get("created_at")),
        reviewed_at: row
            .get::<Option<time::OffsetDateTime>, _>("reviewed_at")
            .map(format_rfc3339),
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    status: Option<String>,
}

#[derive(Deserialize)]
pub struct RejectRequest {
    #[serde(default)]
    reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Inserts a new pending change request. Called by `super::flags::patch_flag`
/// once it has already validated the patch would apply cleanly.
pub(crate) async fn create_change_request(
    db: impl sqlx::PgExecutor<'_>,
    env_id: &str,
    flag_key: &str,
    patch: &serde_json::Value,
    requested_by: &str,
) -> Result<ChangeRequestInfo, StatusCode> {
    let row = sqlx::query(
        "INSERT INTO change_requests (environment_id, flag_key, patch, requested_by) \
         VALUES ($1::uuid, $2, $3, $4) \
         RETURNING id, environment_id::text, flag_key, patch, requested_by, status, \
                   reviewed_by, reason, created_at, reviewed_at",
    )
    .bind(env_id)
    .bind(flag_key)
    .bind(patch)
    .bind(requested_by)
    .fetch_one(db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to create change request");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(row_to_info(&row))
}

/// Locks and returns a pending change request, verifying it belongs to `env_id`.
async fn lock_pending(
    tx: &mut sqlx::PgTransaction<'_>,
    env_id: &str,
    id: i64,
) -> Result<sqlx::postgres::PgRow, StatusCode> {
    sqlx::query(
        "SELECT id, environment_id::text, flag_key, patch, requested_by, status, \
                reviewed_by, reason, created_at, reviewed_at \
         FROM change_requests WHERE id = $1 AND environment_id = $2::uuid FOR UPDATE",
    )
    .bind(id)
    .bind(env_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error locking change request");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/environments/:env_id/change-requests?status=pending
pub async fn list_change_requests(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(env_id): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<ChangeRequestInfo>>, StatusCode> {
    super::flags::check_env_access(&state.db, &jar, &env_id).await?;

    let rows = sqlx::query(
        "SELECT id, environment_id::text, flag_key, patch, requested_by, status, \
                reviewed_by, reason, created_at, reviewed_at \
         FROM change_requests \
         WHERE environment_id = $1::uuid AND ($2::text IS NULL OR status = $2) \
         ORDER BY created_at DESC",
    )
    .bind(&env_id)
    .bind(&q.status)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to list change requests");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(rows.iter().map(row_to_info).collect()))
}

/// POST /api/environments/:env_id/change-requests/:id/approve
///
/// Applies the stored patch and marks the request approved. Blocked if the
/// caller is the original requester — someone else must review it, the same
/// way a PR author can't approve their own PR.
pub async fn approve_change_request(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, id)): Path<(String, i64)>,
) -> Result<Json<super::flags::FlagWithMetadata>, StatusCode> {
    super::flags::check_env_access(&state.db, &jar, &env_id).await?;
    let reviewer = get_session_claims(&jar)
        .map(|c| c.email)
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let mut tx = state.db.begin().await.map_err(|e| {
        error!(error = %e, "Failed to begin transaction");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let row = lock_pending(&mut tx, &env_id, id).await?;
    let status: String = row.get("status");
    if status != "pending" {
        let _ = tx.rollback().await;
        return Err(StatusCode::CONFLICT);
    }

    let requested_by: String = row.get("requested_by");
    if requested_by == reviewer {
        warn!(change_request_id = id, "Rejected self-approval attempt");
        let _ = tx.rollback().await;
        return Err(StatusCode::FORBIDDEN);
    }

    let flag_key: String = row.get("flag_key");
    let patch: serde_json::Value = row.get("patch");
    let applied = super::flags::apply_patch_in_tx(&mut tx, &env_id, &flag_key, patch).await?;

    sqlx::query(
        "UPDATE change_requests SET status = 'approved', reviewed_by = $1, reviewed_at = NOW() \
         WHERE id = $2",
    )
    .bind(&reviewer)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to mark change request approved");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    tx.commit().await.map_err(|e| {
        error!(error = %e, "Failed to commit change-request approval");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let applied =
        super::flags::emit_patch(&state, &env_id, &flag_key, applied, Some(&reviewer), None).await;

    // Note `emit_patch` above already emitted `flag.updated`; this second
    // event records *who reviewed it*, which the flag event doesn't carry.
    crate::notify::notify(
        state.clone(),
        env_id.clone(),
        crate::integrations::change_request_payload(
            "change_request.approved",
            &env_id,
            &flag_key,
            id,
            Some(&reviewer),
            None,
        ),
    );

    info!(change_request_id = id, env_id = %env_id, flag_key = %flag_key, "Change request approved and applied");
    Ok(Json(applied))
}

/// POST /api/environments/:env_id/change-requests/:id/reject
pub async fn reject_change_request(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, id)): Path<(String, i64)>,
    Json(req): Json<RejectRequest>,
) -> Result<StatusCode, StatusCode> {
    super::flags::check_env_access(&state.db, &jar, &env_id).await?;
    let reviewer = get_session_claims(&jar)
        .map(|c| c.email)
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // RETURNING the flag key so the rejection notification can name the flag —
    // "change request rejected" alone isn't actionable in a chat channel.
    let row = sqlx::query(
        "UPDATE change_requests SET status = 'rejected', reviewed_by = $1, reviewed_at = NOW(), reason = $2 \
         WHERE id = $3 AND environment_id = $4::uuid AND status = 'pending' \
         RETURNING flag_key",
    )
    .bind(&reviewer)
    .bind(&req.reason)
    .bind(id)
    .bind(&env_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to reject change request");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let Some(row) = row else {
        return Err(StatusCode::NOT_FOUND);
    };
    let flag_key: String = row.get("flag_key");

    crate::notify::notify(
        state.clone(),
        env_id.clone(),
        crate::integrations::change_request_payload(
            "change_request.rejected",
            &env_id,
            &flag_key,
            id,
            Some(&reviewer),
            req.reason.as_deref(),
        ),
    );

    info!(change_request_id = id, env_id = %env_id, "Change request rejected");
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/environments/:env_id/change-requests/:id — withdraw your own
/// pending request. Admins may cancel anyone's (incident-response override,
/// mirroring how workspace admins bypass project membership elsewhere).
pub async fn cancel_change_request(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, id)): Path<(String, i64)>,
) -> Result<StatusCode, StatusCode> {
    super::flags::check_env_access(&state.db, &jar, &env_id).await?;
    let claims = get_session_claims(&jar);
    let is_admin = claims.as_ref().is_some_and(|c| c.role == "admin");
    let caller_email = claims.map(|c| c.email);

    let result = if is_admin || caller_email.is_none() {
        // Admin, or SDK-key auth (admin-equivalent) — can cancel any pending request.
        sqlx::query(
            "DELETE FROM change_requests \
             WHERE id = $1 AND environment_id = $2::uuid AND status = 'pending'",
        )
        .bind(id)
        .bind(&env_id)
        .execute(&state.db)
        .await
    } else {
        sqlx::query(
            "DELETE FROM change_requests \
             WHERE id = $1 AND environment_id = $2::uuid AND status = 'pending' AND requested_by = $3",
        )
        .bind(id)
        .bind(&env_id)
        .bind(caller_email.as_deref())
        .execute(&state.db)
        .await
    }
    .map_err(|e| {
        error!(error = %e, "Failed to cancel change request");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if result.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    info!(change_request_id = id, env_id = %env_id, "Change request cancelled");
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Routers
// ---------------------------------------------------------------------------

/// Read-only — any authenticated user with environment access.
pub fn read_router() -> Router<AppState> {
    Router::new().route(
        "/environments/{env_id}/change-requests",
        get(list_change_requests),
    )
}

/// Editor/admin-gated — same tier as flag writes, since approving/rejecting/
/// cancelling a change request is itself a flag-adjacent write action.
pub fn write_router() -> Router<AppState> {
    Router::new()
        .route(
            "/environments/{env_id}/change-requests/{id}/approve",
            post(approve_change_request),
        )
        .route(
            "/environments/{env_id}/change-requests/{id}/reject",
            post(reject_change_request),
        )
        .route(
            "/environments/{env_id}/change-requests/{id}",
            delete(cancel_change_request),
        )
}
