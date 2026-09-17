use crate::auth::{AuthContext, get_session_claims};
use crate::state::AppState;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use checkgate_core::evaluator::Flag;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;
use tracing::{error, info, instrument, warn};

// ---------------------------------------------------------------------------
// Lifecycle metadata (tags / ownership / archival)
// ---------------------------------------------------------------------------
//
// Tags, owner, and archival state are management/UI metadata, not evaluation
// inputs — kept as discrete `flags` table columns rather than inside `data`
// so they never flow into the evaluation core or over SSE to SDK clients.
// `core::Flag` (and therefore the SSE/`/flags/snapshot` wire format) is
// untouched by this feature.

/// A flag as returned by the dashboard-facing REST API — the evaluation `Flag`
/// plus lifecycle metadata. Never sent over SSE or `/flags/snapshot`; those
/// paths serialize bare `Flag` values read directly from the `data` column.
#[derive(Debug, Serialize)]
pub struct FlagWithMetadata {
    #[serde(flatten)]
    pub flag: Flag,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    /// `time`'s default (de)serialization is a proprietary space-separated,
    /// triple-colon-offset format that JavaScript's `Date` cannot parse —
    /// `time::serde::rfc3339` gives proper RFC 3339 (`"2026-07-04T03:19:17Z"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub archived_at: Option<time::OffsetDateTime>,
}

/// Request body for `POST /flags` — the evaluation `Flag` plus optional
/// lifecycle metadata set at creation time.
#[derive(Debug, Deserialize)]
struct CreateFlagRequest {
    #[serde(flatten)]
    flag: Flag,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    owner_email: Option<String>,
}

// ---------------------------------------------------------------------------
// Environment-scoped path params
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct EnvFlagPath {
    env_id: String,
    key: String,
}

// ---------------------------------------------------------------------------
// Project access helper
// ---------------------------------------------------------------------------

/// Verifies the caller can access the project that owns `env_id`.
/// Workspace admins and SDK key auth always pass.
/// Others must be a project member.
pub(super) async fn check_env_access(
    db: &sqlx::PgPool,
    jar: &AuthContext,
    env_id: &str,
) -> Result<(), StatusCode> {
    if let Some(claims) = get_session_claims(jar) {
        crate::auth::check_env_role(db, &claims, env_id, false).await?;
    }
    Ok(())
}

pub(super) async fn check_env_write_access(
    db: &sqlx::PgPool,
    jar: &AuthContext,
    env_id: &str,
) -> Result<(), StatusCode> {
    if let Some(claims) = get_session_claims(jar) {
        crate::auth::check_env_role(db, &claims, env_id, true).await?;
    }
    Ok(())
}

/// Serialize approval-policy changes with mutations of this environment.
pub(crate) async fn lock_environment(
    tx: &mut sqlx::PgTransaction<'_>,
    env_id: &str,
) -> Result<bool, StatusCode> {
    sqlx::query_scalar("SELECT require_approval FROM environments WHERE id = $1::uuid FOR SHARE")
        .bind(env_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|e| {
            error!(error = %e, "Environment lock failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Flag keys must be non-empty, at most 100 characters, and contain only
/// ASCII alphanumerics, underscores, or hyphens. This prevents ambiguous
/// routing, log pollution, and surprises in SDK consumers that use the key
/// as a cache key or filename.
fn is_valid_flag_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 100
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

// ---------------------------------------------------------------------------
// Routers
// ---------------------------------------------------------------------------

/// Read-only routes — available to any authenticated user (admin or viewer).
pub fn read_router() -> Router<AppState> {
    Router::new()
        .route("/environments/{env_id}/flags", get(list_flags))
        .route("/environments/{env_id}/flags/{key}", get(get_flag))
}

pub fn write_router() -> Router<AppState> {
    Router::new()
        .route("/environments/{env_id}/flags", post(create_flag))
        .route(
            "/environments/{env_id}/flags/{key}",
            axum::routing::delete(delete_flag).patch(patch_flag),
        )
        .route(
            "/environments/{env_id}/flags/{key}/promote",
            post(promote_flag),
        )
        .route(
            "/environments/{env_id}/flags/{key}/archive",
            post(archive_flag),
        )
        .route(
            "/environments/{env_id}/flags/{key}/unarchive",
            post(unarchive_flag),
        )
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Pagination {
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
    /// Include archived flags in the results. Defaults to false — archived
    /// flags are hidden from the default dashboard list view.
    #[serde(default)]
    include_archived: bool,
    /// Filter to flags carrying this exact tag.
    #[serde(default)]
    tag: Option<String>,
}

fn default_limit() -> usize {
    200
}

/// Hard ceiling on `limit`, whatever the client asks for. Two reasons: an
/// uncapped `limit` lets one authenticated request pull the entire flag table,
/// and a `usize` too large for `i64` used to wrap to a negative `LIMIT`, which
/// Postgres rejects with a 500 rather than a 4xx.
const MAX_PAGE_LIMIT: i64 = 500;

/// Ceiling on `offset`. Paging deeper than this is a sequential scan, not a query.
const MAX_PAGE_OFFSET: i64 = 1_000_000;

impl Pagination {
    /// `limit` clamped into `0..=MAX_PAGE_LIMIT`, as the `i64` Postgres binds.
    fn limit_i64(&self) -> i64 {
        i64::try_from(self.limit)
            .unwrap_or(MAX_PAGE_LIMIT)
            .clamp(0, MAX_PAGE_LIMIT)
    }

    /// `offset` clamped into `0..=MAX_PAGE_OFFSET`.
    fn offset_i64(&self) -> i64 {
        i64::try_from(self.offset)
            .unwrap_or(MAX_PAGE_OFFSET)
            .clamp(0, MAX_PAGE_OFFSET)
    }
}

/// Builds a `FlagWithMetadata` from a `flags` row that selected
/// `data, tags, owner_email, archived_at`. Returns `None` if `data` doesn't
/// deserialize into a valid `Flag` (defensive — should not happen for rows
/// written by this server).
fn row_to_flag_with_metadata(row: &sqlx::postgres::PgRow) -> Option<FlagWithMetadata> {
    let data: serde_json::Value = row.try_get("data").ok()?;
    let flag: Flag = serde_json::from_value(data).ok()?;
    Some(FlagWithMetadata {
        flag,
        tags: row.try_get("tags").unwrap_or_default(),
        owner_email: row.try_get("owner_email").unwrap_or(None),
        archived_at: row.try_get("archived_at").unwrap_or(None),
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[instrument(skip(state, jar))]
async fn list_flags(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(env_id): Path<String>,
    Query(page): Query<Pagination>,
) -> Result<Json<Vec<FlagWithMetadata>>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    let rows = sqlx::query(
        "SELECT data, tags, owner_email, archived_at FROM flags \
         WHERE environment_id = $1::uuid \
           AND ($4 OR archived_at IS NULL) \
           AND ($5::text IS NULL OR tags @> ARRAY[$5]) \
         ORDER BY key ASC LIMIT $2 OFFSET $3",
    )
    .bind(&env_id)
    .bind(page.limit_i64())
    .bind(page.offset_i64())
    .bind(page.include_archived)
    .bind(page.tag.as_deref())
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to list flags");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let flags: Vec<FlagWithMetadata> = rows.iter().filter_map(row_to_flag_with_metadata).collect();

    info!(
        count = flags.len(),
        offset = page.offset,
        limit = page.limit,
        env_id = %env_id,
        "Listed flags"
    );
    Ok(Json(flags))
}

#[instrument(skip(state, jar, req), fields(flag_key = %req.flag.key))]
async fn create_flag(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(env_id): Path<String>,
    Json(req): Json<CreateFlagRequest>,
) -> Result<Json<FlagWithMetadata>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;
    let payload = req.flag;
    if !is_valid_flag_key(&payload.key) {
        warn!(key = %payload.key, "Rejected create_flag: invalid key");
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    if payload.rollout_percentage.is_some_and(|p| p > 100) {
        warn!(
            rollout_percentage = ?payload.rollout_percentage,
            "Rejected create_flag: rollout_percentage out of range"
        );
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let data = serde_json::to_value(&payload).map_err(|e| {
        error!(error = %e, "Failed to serialize flag for DB write");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let protected = lock_environment(&mut tx, &env_id).await?;
    let written = sqlx::query(
        "INSERT INTO flags (key, environment_id, data, tags, owner_email) \
         VALUES ($1, $2::uuid, $3, $4, $5) \
         ON CONFLICT (key, environment_id) \
         DO UPDATE SET data = EXCLUDED.data, tags = EXCLUDED.tags, owner_email = EXCLUDED.owner_email WHERE NOT $6",
    )
    .bind(&payload.key)
    .bind(&env_id)
    .bind(&data)
    .bind(&req.tags)
    .bind(&req.owner_email)
    .bind(protected)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        error!(error = %e, "PostgreSQL write failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if written.rows_affected() == 0 {
        return Err(StatusCode::CONFLICT);
    }
    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let actor_email = get_session_claims(&jar).map(|c| c.email);
    super::audit::log_audit_event(
        &state.db,
        &env_id,
        &payload.key,
        actor_email.as_deref(),
        "CREATE",
        None,
        Some(&data),
        None,
    )
    .await;

    let segment_map = super::segments::load_env_segments(&env_id, &state.db)
        .await
        .unwrap_or_default();
    let expanded = super::segments::expand_flag_with_segments(payload.clone(), &segment_map);
    state.store.upsert_flag(expanded.clone());
    let msg = json!({"type": "UPSERT", "env_id": env_id, "flag": expanded}).to_string();
    publish_update(&state, &msg, "create_flag").await;

    let wh_payload = super::webhooks::flag_event_payload(
        "flag.created",
        &env_id,
        &payload.key,
        Some(&data),
        actor_email.as_deref(),
        None,
    );
    crate::notify::notify(state.clone(), env_id.clone(), wh_payload);

    info!(env_id = %env_id, "Flag created/replaced");
    Ok(Json(FlagWithMetadata {
        flag: payload,
        tags: req.tags,
        owner_email: req.owner_email,
        archived_at: None,
    }))
}

#[instrument(skip(state, jar), fields(flag_key = %path.key))]
async fn get_flag(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(path): Path<EnvFlagPath>,
) -> Result<Json<FlagWithMetadata>, StatusCode> {
    check_env_access(&state.db, &jar, &path.env_id).await?;
    let row = sqlx::query(
        "SELECT data, tags, owner_email, archived_at FROM flags \
         WHERE key = $1 AND environment_id = $2::uuid",
    )
    .bind(&path.key)
    .bind(&path.env_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error fetching flag");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

    let flag = row_to_flag_with_metadata(&row).ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;

    info!(env_id = %path.env_id, "Flag retrieved");
    Ok(Json(flag))
}

#[instrument(skip(state, jar), fields(flag_key = %path.key))]
async fn delete_flag(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(path): Path<EnvFlagPath>,
) -> Result<StatusCode, StatusCode> {
    check_env_access(&state.db, &jar, &path.env_id).await?;

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if lock_environment(&mut tx, &path.env_id).await? {
        return Err(StatusCode::CONFLICT);
    }

    // RETURNING captures before_data for the audit log atomically with the delete.
    let row = sqlx::query(
        "DELETE FROM flags WHERE key = $1 AND environment_id = $2::uuid RETURNING data",
    )
    .bind(&path.key)
    .bind(&path.env_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| {
        error!(error = %e, "PostgreSQL delete failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let before_data: Option<serde_json::Value> = row.try_get("data").ok();
    let actor_email = get_session_claims(&jar).map(|c| c.email);
    super::audit::log_audit_event(
        &state.db,
        &path.env_id,
        &path.key,
        actor_email.as_deref(),
        "DELETE",
        before_data.as_ref(),
        None,
        None,
    )
    .await;

    let msg = json!({"type": "DELETE", "env_id": path.env_id, "key": path.key}).to_string();
    publish_update(&state, &msg, "delete_flag").await;

    let wh_payload = super::webhooks::flag_event_payload(
        "flag.deleted",
        &path.env_id,
        &path.key,
        before_data.as_ref(),
        actor_email.as_deref(),
        None,
    );
    crate::notify::notify(state.clone(), path.env_id.clone(), wh_payload);

    info!(env_id = %path.env_id, "Flag deleted");
    Ok(StatusCode::NO_CONTENT)
}

/// Merges `patch` onto the flag JSON currently stored for `key`/`env_id` and
/// validates the result deserializes to a valid [`Flag`] with an in-range
/// rollout — without writing anything. Shared by the direct-apply path and
/// the change-request approval path so both reject the same way, and by the
/// approval-required path so a request is only queued if it would actually
/// apply cleanly.
///
/// Also pulls `tags`/`owner_email` (and drops `key`) out of the patch, since
/// those are discrete columns, not part of `data` — see the module-level
/// comment on [`FlagWithMetadata`].
pub(super) async fn merge_and_validate(
    db: impl sqlx::PgExecutor<'_>,
    env_id: &str,
    key: &str,
    mut patch: serde_json::Value,
) -> Result<
    (
        serde_json::Value,
        serde_json::Value,
        Flag,
        Option<Vec<String>>,
        Option<Option<String>>,
    ),
    StatusCode,
> {
    if !patch.is_object() {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let (mut new_tags, mut new_owner_email) = (None, None);
    if let serde_json::Value::Object(ref mut m) = patch {
        m.remove("key");
        if let Some(v) = m.remove("tags") {
            new_tags = Some(
                serde_json::from_value::<Vec<String>>(v)
                    .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?,
            );
        }
        if let Some(v) = m.remove("owner_email") {
            if !v.is_null() && !v.is_string() {
                return Err(StatusCode::UNPROCESSABLE_ENTITY);
            }
            new_owner_email = Some(v.as_str().map(str::to_string));
        }
    }

    let before_val: serde_json::Value =
        sqlx::query_scalar("SELECT data FROM flags WHERE key = $1 AND environment_id = $2::uuid")
            .bind(key)
            .bind(env_id)
            .fetch_optional(db)
            .await
            .map_err(|e| {
                error!(error = %e, "PostgreSQL read failed");
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .ok_or_else(|| {
                info!("Flag not found for PATCH");
                StatusCode::NOT_FOUND
            })?;

    let mut flag_val = before_val.clone();
    if let (serde_json::Value::Object(map), serde_json::Value::Object(patch_map)) =
        (&mut flag_val, patch)
    {
        for (k, v) in patch_map {
            map.insert(k, v);
        }
    }

    let flag: Flag = serde_json::from_value(flag_val.clone()).map_err(|e| {
        error!(error = %e, "Merged flag is not a valid Flag — patch rejected");
        StatusCode::UNPROCESSABLE_ENTITY
    })?;

    if flag.rollout_percentage.is_some_and(|p| p > 100) {
        warn!(
            rollout_percentage = ?flag.rollout_percentage,
            "Rejected patch_flag: rollout_percentage out of range"
        );
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    Ok((before_val, flag_val, flag, new_tags, new_owner_email))
}

/// Flag mutation and its before/after values, held until the enclosing transaction commits.
pub(crate) struct AppliedPatch {
    pub result: FlagWithMetadata,
    pub before: serde_json::Value,
    pub data: serde_json::Value,
}

pub(crate) async fn apply_patch_in_tx(
    db_tx: &mut sqlx::PgTransaction<'_>,
    env_id: &str,
    key: &str,
    patch: serde_json::Value,
) -> Result<AppliedPatch, StatusCode> {
    // Re-select FOR UPDATE inside the write transaction — `merge_and_validate`
    // above may have run against a plain (non-locking) read for a dry-run.
    let rec = sqlx::query(
        "SELECT tags, owner_email, archived_at FROM flags \
         WHERE key = $1 AND environment_id = $2::uuid FOR UPDATE",
    )
    .bind(key)
    .bind(env_id)
    .fetch_optional(&mut **db_tx)
    .await
    .map_err(|e| {
        error!(error = %e, "PostgreSQL read failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

    let (before_val, flag_val, flag, new_tags, new_owner_email) =
        merge_and_validate(&mut **db_tx, env_id, key, patch).await?;

    // Only touch tags/owner_email if the patch provided them — otherwise keep
    // whatever is already stored (read above under the same row lock).
    let tags: Vec<String> = new_tags.unwrap_or(rec.try_get("tags").unwrap_or_default());
    let owner_email: Option<String> =
        new_owner_email.unwrap_or(rec.try_get("owner_email").unwrap_or(None));
    let archived_at: Option<time::OffsetDateTime> = rec.try_get("archived_at").unwrap_or(None);

    sqlx::query(
        "UPDATE flags SET data = $1, tags = $2, owner_email = $3 \
         WHERE key = $4 AND environment_id = $5::uuid",
    )
    .bind(&flag_val)
    .bind(&tags)
    .bind(&owner_email)
    .bind(key)
    .bind(env_id)
    .execute(&mut **db_tx)
    .await
    .map_err(|e| {
        error!(error = %e, "PostgreSQL update failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(AppliedPatch {
        result: FlagWithMetadata {
            flag,
            tags,
            owner_email,
            archived_at,
        },
        before: before_val,
        data: flag_val,
    })
}

/// Emit side effects only after the flag and workflow status have committed.
pub(crate) async fn emit_patch(
    state: &AppState,
    env_id: &str,
    key: &str,
    applied: AppliedPatch,
    actor_email: Option<&str>,
    scheduled_change_id: Option<&str>,
) -> FlagWithMetadata {
    let metadata = scheduled_change_id.map(|id| json!({"scheduled_change_id": id}));
    super::audit::log_audit_event(
        &state.db,
        env_id,
        key,
        actor_email,
        "UPDATE",
        Some(&applied.before),
        Some(&applied.data),
        metadata.as_ref(),
    )
    .await;

    let segment_map = super::segments::load_env_segments(env_id, &state.db)
        .await
        .unwrap_or_default();
    let expanded =
        super::segments::expand_flag_with_segments(applied.result.flag.clone(), &segment_map);
    state.store.upsert_flag(expanded.clone());
    let msg = json!({"type": "UPSERT", "env_id": env_id, "flag": expanded}).to_string();
    publish_update(state, &msg, "patch_flag").await;

    let wh_payload = super::webhooks::flag_event_payload(
        if scheduled_change_id.is_some() {
            "flag.scheduled_change_applied"
        } else {
            "flag.updated"
        },
        env_id,
        key,
        Some(&applied.data),
        actor_email,
        metadata.as_ref(),
    );
    crate::notify::notify(state.clone(), env_id.to_string(), wh_payload);

    applied.result
}

/// PATCH /api/environments/:env_id/flags/:key — partial update via JSON merge.
///
/// Only provided fields are changed; omitted fields retain their current values.
/// The `key` field is excluded from the patch to prevent key aliasing.
///
/// If the environment has `require_approval` set, the patch is not applied —
/// it's captured as a pending [`super::change_requests::ChangeRequestInfo`]
/// and `202 Accepted` is returned instead of `200 OK`. The patch is validated
/// (would it produce a valid flag?) before being queued, so approval doesn't
/// surface a validation error later on someone else's click.
#[instrument(skip(state, jar, patch), fields(flag_key = %path.key))]
async fn patch_flag(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(path): Path<EnvFlagPath>,
    Json(patch): Json<serde_json::Value>,
) -> Result<axum::response::Response, StatusCode> {
    use axum::response::IntoResponse;

    check_env_access(&state.db, &jar, &path.env_id).await?;

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let require_approval = lock_environment(&mut tx, &path.env_id).await?;
    let actor_email = get_session_claims(&jar).map(|c| c.email);
    if !require_approval {
        let applied = apply_patch_in_tx(&mut tx, &path.env_id, &path.key, patch).await?;
        tx.commit()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let updated = emit_patch(
            &state,
            &path.env_id,
            &path.key,
            applied,
            actor_email.as_deref(),
            None,
        )
        .await;
        return Ok(Json(updated).into_response());
    }
    let requested_by = actor_email.ok_or(StatusCode::UNAUTHORIZED)?;
    merge_and_validate(&mut *tx, &path.env_id, &path.key, patch.clone()).await?;
    let cr = super::change_requests::create_change_request(
        &mut *tx,
        &path.env_id,
        &path.key,
        &patch,
        &requested_by,
    )
    .await?;
    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // A queued request is waiting on a human, so it's the event most worth
    // surfacing in chat — nothing applies until someone reviews it.
    crate::notify::notify(
        state.clone(),
        path.env_id.clone(),
        crate::integrations::change_request_payload(
            "change_request.opened",
            &path.env_id,
            &path.key,
            cr.id,
            Some(&requested_by),
            None,
        ),
    );

    info!(env_id = %path.env_id, flag_key = %path.key, "Flag patch queued for approval");
    Ok((StatusCode::ACCEPTED, Json(cr)).into_response())
}

/// POST /api/environments/:env_id/flags/:key/promote
///
/// Copies a flag's configuration from one environment to another (the target
/// environment is specified in the JSON body as `target_env_id`).
/// This is the "promote to production" flow.
#[instrument(skip(state, jar), fields(flag_key = %path.key))]
async fn promote_flag(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(path): Path<EnvFlagPath>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<Flag>, StatusCode> {
    check_env_access(&state.db, &jar, &path.env_id).await?;
    let target_env_id = body
        .get("target_env_id")
        .and_then(|v| v.as_str())
        .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?
        .to_string();
    check_env_write_access(&state.db, &jar, &target_env_id).await?;

    let mut db_tx = state.db.begin().await.map_err(|e| {
        error!(error = %e, "Failed to begin transaction");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if lock_environment(&mut db_tx, &target_env_id).await? {
        return Err(StatusCode::CONFLICT);
    }

    // Read source flag.
    let rec = sqlx::query("SELECT data FROM flags WHERE key = $1 AND environment_id = $2::uuid")
        .bind(&path.key)
        .bind(&path.env_id)
        .fetch_optional(&mut *db_tx)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to read source flag for promote");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let flag_val: serde_json::Value = rec
        .try_get("data")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let flag: Flag =
        serde_json::from_value(flag_val.clone()).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Upsert into target environment.
    sqlx::query(
        "INSERT INTO flags (key, environment_id, data) VALUES ($1, $2::uuid, $3) \
         ON CONFLICT (key, environment_id) DO UPDATE SET data = EXCLUDED.data",
    )
    .bind(&path.key)
    .bind(&target_env_id)
    .bind(&flag_val)
    .execute(&mut *db_tx)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to write promoted flag");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    db_tx.commit().await.map_err(|e| {
        error!(error = %e, "Transaction commit failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let actor_email = get_session_claims(&jar).map(|c| c.email);
    let promote_meta = serde_json::json!({"from_env_id": path.env_id, "to_env_id": target_env_id});
    super::audit::log_audit_event(
        &state.db,
        &target_env_id,
        &path.key,
        actor_email.as_deref(),
        "PROMOTE",
        None,
        Some(&flag_val),
        Some(&promote_meta),
    )
    .await;

    let segment_map = super::segments::load_env_segments(&target_env_id, &state.db)
        .await
        .unwrap_or_default();
    let expanded = super::segments::expand_flag_with_segments(flag.clone(), &segment_map);
    state.store.upsert_flag(expanded.clone());
    let msg = json!({"type": "UPSERT", "env_id": target_env_id, "flag": expanded}).to_string();
    publish_update(&state, &msg, "promote_flag").await;

    let wh_payload = super::webhooks::flag_event_payload(
        "flag.promoted",
        &target_env_id,
        &path.key,
        Some(&flag_val),
        actor_email.as_deref(),
        Some(&promote_meta),
    );
    crate::notify::notify(state.clone(), target_env_id.clone(), wh_payload);

    info!(
        from_env = %path.env_id,
        to_env = %target_env_id,
        "Flag promoted"
    );
    Ok(Json(flag))
}

/// POST /api/environments/:env_id/flags/:key/archive
/// POST /api/environments/:env_id/flags/:key/unarchive
///
/// Archiving is a pure dashboard/management concept — it hides a flag from
/// the default list view to help teams find flags that are safe to clean up.
/// It has **no effect on evaluation**: `is_enabled`/`rollout_percentage`
/// remain the only kill-switches, so an archived flag keeps behaving exactly
/// as before for any client still evaluating it. No SSE update or webhook is
/// fired, since nothing evaluation-relevant changed.
async fn set_archived(
    state: &AppState,
    jar: &AuthContext,
    path: &EnvFlagPath,
    archived: bool,
) -> Result<Json<FlagWithMetadata>, StatusCode> {
    check_env_access(&state.db, jar, &path.env_id).await?;

    let row = sqlx::query(
        "UPDATE flags SET archived_at = CASE WHEN $3 THEN NOW() ELSE NULL END \
         WHERE key = $1 AND environment_id = $2::uuid \
         RETURNING data, tags, owner_email, archived_at",
    )
    .bind(&path.key)
    .bind(&path.env_id)
    .bind(archived)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "PostgreSQL update failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

    let flag_with_metadata =
        row_to_flag_with_metadata(&row).ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;

    let actor_email = get_session_claims(jar).map(|c| c.email);
    let metadata = json!({ "archived": archived });
    super::audit::log_audit_event(
        &state.db,
        &path.env_id,
        &path.key,
        actor_email.as_deref(),
        if archived { "ARCHIVE" } else { "UNARCHIVE" },
        None,
        None,
        Some(&metadata),
    )
    .await;

    info!(env_id = %path.env_id, archived, "Flag archive state changed");
    Ok(Json(flag_with_metadata))
}

#[instrument(skip(state, jar), fields(flag_key = %path.key))]
async fn archive_flag(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(path): Path<EnvFlagPath>,
) -> Result<Json<FlagWithMetadata>, StatusCode> {
    set_archived(&state, &jar, &path, true).await
}

#[instrument(skip(state, jar), fields(flag_key = %path.key))]
async fn unarchive_flag(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(path): Path<EnvFlagPath>,
) -> Result<Json<FlagWithMetadata>, StatusCode> {
    set_archived(&state, &jar, &path, false).await
}

// ---------------------------------------------------------------------------
// Redis helpers
// ---------------------------------------------------------------------------

/// Publish a flag change event using the shared multiplexed connection from
/// AppState. If Redis is unavailable the DB write already succeeded so the
/// request is not failed, but other instances may serve stale data until their
/// next SSE reconnect.
pub(crate) async fn publish_update(state: &AppState, msg: &str, op: &str) {
    let mut conn = state.redis_conn.clone();
    if let Err(e) = conn.publish::<_, _, ()>("checkgate_updates", msg).await {
        warn!(
            error = %e,
            operation = op,
            "Redis publish failed — other instances may be stale"
        );
    }
}
