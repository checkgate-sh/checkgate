use crate::auth::{AuthContext, get_session_claims};
use crate::state::{AppState, SdkKeyEntry};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use axum_extra::extract::cookie::PrivateCookieJar;
use rand::RngExt as _;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tracing::{error, info};

/// Formats a timestamp as RFC 3339 for JSON responses. `OffsetDateTime`'s
/// `Display`/`to_string()` uses a proprietary space-separated, triple-colon-offset
/// format that JavaScript's `Date` cannot parse.
fn format_rfc3339(dt: time::OffsetDateTime) -> String {
    dt.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| dt.to_string())
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Safe to return in list responses — does NOT include the key value.
#[derive(Serialize)]
pub struct SdkKeyInfo {
    pub id: i64,
    pub name: String,
    /// First 16 chars of the key + "…" — safe to display.
    pub prefix: String,
    pub environment_id: String,
    pub environment_name: String,
    /// ISO-8601 string.
    pub created_at: String,
}

/// Returned only once: when the key is first created or during initial setup.
#[derive(Serialize)]
pub struct NewKeyResponse {
    pub id: i64,
    pub name: String,
    /// Full key value — shown only once, never again.
    pub key: String,
    pub prefix: String,
    pub environment_id: String,
    pub environment_name: String,
    /// ISO-8601 string.
    pub created_at: String,
}

#[derive(Deserialize)]
pub struct CreateKeyRequest {
    pub name: String,
    pub environment_id: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(crate) fn generate_sdk_key() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("sk_live_{hex}")
}

fn prefix_of(key: &str) -> String {
    format!("{}…", key.chars().take(16).collect::<String>())
}

// ---------------------------------------------------------------------------
// Access helpers (project-level)
// ---------------------------------------------------------------------------

async fn can_access_project(
    db: &sqlx::PgPool,
    jar: &AuthContext,
    project_id: &str,
) -> Result<(), StatusCode> {
    let Some(claims) = get_session_claims(jar) else {
        return Err(StatusCode::FORBIDDEN); // SDK keys are confined to the SDK routes
    };
    if claims.role == "admin" {
        return Ok(());
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM project_members pm JOIN users u ON u.id = pm.user_id \
         WHERE pm.project_id = $1::uuid AND u.email = $2)",
    )
    .bind(project_id)
    .bind(&claims.email)
    .fetch_one(db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error checking project membership");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if exists {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

async fn can_admin_project(
    db: &sqlx::PgPool,
    jar: &AuthContext,
    project_id: &str,
) -> Result<(), StatusCode> {
    let Some(claims) = get_session_claims(jar) else {
        return Err(StatusCode::FORBIDDEN); // SDK keys are confined to the SDK routes
    };
    if claims.role == "admin" {
        return Ok(());
    }
    let role: Option<String> = sqlx::query_scalar(
        "SELECT pm.role FROM project_members pm \
         JOIN users u ON u.id = pm.user_id \
         WHERE pm.project_id = $1::uuid AND u.email = $2",
    )
    .bind(project_id)
    .bind(&claims.email)
    .fetch_optional(db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error checking project admin");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    match role.as_deref() {
        Some("admin") => Ok(()),
        _ => Err(StatusCode::FORBIDDEN),
    }
}

// ---------------------------------------------------------------------------
// Public setup route — no auth required
// ---------------------------------------------------------------------------

/// Returns the auto-generated initial SDK key.
///
/// Only works before setup is complete (`settings.setup_complete` not set).
/// Once the first login happens, this endpoint returns 404.
pub async fn get_setup_key(
    State(state): State<AppState>,
    jar: PrivateCookieJar,
    headers: axum::http::HeaderMap,
) -> Result<Json<NewKeyResponse>, StatusCode> {
    if jar.get("lg_session").is_some() {
        return Err(StatusCode::NOT_FOUND);
    }

    if crate::setup::is_setup_complete(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Err(StatusCode::NOT_FOUND);
    }
    // The key is only shown to whoever is entitled to claim the installation.
    crate::setup::verify(&state, &headers).await?;

    let row = sqlx::query(
        "SELECT k.id, k.name, k.value, k.created_at, k.environment_id::text, e.name AS env_name \
         FROM sdk_keys k JOIN environments e ON e.id = k.environment_id \
         ORDER BY k.id ASC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    let value: String = row.get("value");
    let prefix = prefix_of(&value);
    let created_at: time::OffsetDateTime = row.get("created_at");

    Ok(Json(NewKeyResponse {
        id: row.get("id"),
        name: row.get("name"),
        key: value,
        prefix,
        environment_id: row.get("environment_id"),
        environment_name: row.get("env_name"),
        created_at: format_rfc3339(created_at),
    }))
}

// ---------------------------------------------------------------------------
// Protected routes — require auth + project access
// ---------------------------------------------------------------------------

pub async fn list_keys(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<SdkKeyInfo>>, StatusCode> {
    can_access_project(&state.db, &jar, &project_id).await?;

    let rows = sqlx::query(
        "SELECT k.id, k.name, k.value, k.created_at, k.environment_id::text, e.name AS env_name \
         FROM sdk_keys k \
         JOIN environments e ON e.id = k.environment_id \
         WHERE e.project_id = $1::uuid \
         ORDER BY k.created_at ASC",
    )
    .bind(&project_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to list SDK keys");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let keys = rows
        .iter()
        .map(|row| {
            let value: String = row.get("value");
            let created_at: time::OffsetDateTime = row.get("created_at");
            SdkKeyInfo {
                id: row.get("id"),
                name: row.get("name"),
                prefix: prefix_of(&value),
                environment_id: row.get("environment_id"),
                environment_name: row.get("env_name"),
                created_at: format_rfc3339(created_at),
            }
        })
        .collect();

    Ok(Json(keys))
}

pub async fn create_key(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(project_id): Path<String>,
    Json(req): Json<CreateKeyRequest>,
) -> Result<Json<NewKeyResponse>, StatusCode> {
    can_admin_project(&state.db, &jar, &project_id).await?;

    let name = req.name.trim().to_string();
    if name.is_empty() || name.len() > 100 {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    // Verify the environment belongs to this project.
    let env_name: Option<String> = sqlx::query_scalar(
        "SELECT name FROM environments WHERE id = $1::uuid AND project_id = $2::uuid",
    )
    .bind(&req.environment_id)
    .bind(&project_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error verifying environment ownership");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let env_name = env_name.ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;

    let value = generate_sdk_key();
    let prefix = prefix_of(&value);

    let row = sqlx::query(
        "INSERT INTO sdk_keys (name, value, environment_id) VALUES ($1, $2, $3::uuid) \
         RETURNING id, created_at",
    )
    .bind(&name)
    .bind(&value)
    .bind(&req.environment_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to create SDK key");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let id: i64 = row.get("id");
    let created_at: time::OffsetDateTime = row.get("created_at");

    // Update in-memory cache.
    state.sdk_keys.write().await.push(SdkKeyEntry {
        id,
        name: name.clone(),
        value: value.clone(),
        environment_id: Some(req.environment_id.clone()),
    });

    info!(key_id = id, name = %name, environment_id = %req.environment_id, "SDK key created");

    Ok(Json(NewKeyResponse {
        id,
        name,
        key: value,
        prefix,
        environment_id: req.environment_id,
        environment_name: env_name,
        created_at: format_rfc3339(created_at),
    }))
}

pub async fn revoke_key(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((project_id, key_id)): Path<(String, i64)>,
) -> Result<StatusCode, StatusCode> {
    can_admin_project(&state.db, &jar, &project_id).await?;

    let mut tx = state.db.begin().await.map_err(|e| {
        error!(error = %e, "Failed to begin revoke transaction");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Prevent revoking the last key globally (sdk_key auth would break).
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (SELECT id FROM sdk_keys ORDER BY id FOR UPDATE) AS locked",
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to count SDK keys");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if count <= 1 {
        let _ = tx.rollback().await;
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    // Verify the key belongs to an environment in this project.
    let belongs: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sdk_keys k \
         JOIN environments e ON e.id = k.environment_id \
         WHERE k.id = $1 AND e.project_id = $2::uuid)",
    )
    .bind(key_id)
    .bind(&project_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error verifying key ownership");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if !belongs {
        let _ = tx.rollback().await;
        return Err(StatusCode::NOT_FOUND);
    }

    let revoked_env: Option<String> =
        sqlx::query_scalar("DELETE FROM sdk_keys WHERE id = $1 RETURNING environment_id::text")
            .bind(key_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to revoke SDK key");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

    let Some(env_id) = revoked_env else {
        let _ = tx.rollback().await;
        return Err(StatusCode::NOT_FOUND);
    };

    tx.commit().await.map_err(|e| {
        error!(error = %e, "Failed to commit revoke transaction");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    state.sdk_keys.write().await.retain(|e| e.id != key_id);
    close_streams_of_environment(&state, env_id).await;

    info!(key_id = %key_id, project_id = %project_id, "SDK key revoked");
    Ok(StatusCode::NO_CONTENT)
}

/// Streams authenticate once, when they open, so a revoked key's open
/// streams would keep receiving updates until they reconnect. The stream does
/// not know which key opened it beyond its environment, so the environment's
/// streams are resynced: each closes within `SSE_RESYNC_SPREAD_MS` and its
/// client reconnects. The revoked key's reconnect is refused (401, terminal in
/// the SDKs); the others re-bootstrap. Here directly, and through Redis on
/// every other instance (v0.1.30 instances read the same scoped message). If
/// Redis is unavailable, other instances' streams close on their next
/// reconnect, as before.
async fn close_streams_of_environment(state: &AppState, env_id: String) {
    let event = crate::sync::SyncEvent::resync_env(env_id);
    state
        .metrics
        .resync(crate::metrics::ResyncTrigger::key_revoked);
    let mut conn = state.redis_conn.clone();
    let published = tokio::time::timeout(
        std::time::Duration::from_millis(1500),
        redis::AsyncCommands::publish::<_, _, ()>(
            &mut conn,
            crate::sync::updates_channel(),
            event.payload.as_str(),
        ),
    )
    .await;
    if !matches!(published, Ok(Ok(()))) {
        tracing::warn!(
            "SDK key revoked, but the resync could not be published to Redis: \
             streams on other instances close on their next reconnect"
        );
    }
    crate::sync::relay(state, event);
}

// ---------------------------------------------------------------------------
// Routers
// ---------------------------------------------------------------------------

/// Public — mounted without auth middleware (setup wizard).
pub fn setup_router() -> Router<AppState> {
    Router::new().route("/setup/key", get(get_setup_key))
}

/// Routes nested under /projects/{project_id}/keys.
pub fn read_router() -> Router<AppState> {
    Router::new().route("/projects/{project_id}/keys", get(list_keys))
}

pub fn write_router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project_id}/keys", post(create_key))
        .route("/projects/{project_id}/keys/{id}", delete(revoke_key))
}
