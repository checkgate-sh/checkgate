use crate::auth::AuthContext;
use crate::state::AppState;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
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
pub struct Webhook {
    pub id: String,
    pub environment_id: String,
    pub name: String,
    pub url: String,
    /// Secret is never sent to the client after creation.
    pub has_secret: bool,
    pub enabled: bool,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct WebhookDelivery {
    pub id: i64,
    pub webhook_id: String,
    pub event: String,
    pub status_code: Option<i32>,
    pub response_body: Option<String>,
    pub error: Option<String>,
    pub delivered_at: String,
}

#[derive(Deserialize)]
struct CreateBody {
    name: String,
    url: String,
    #[serde(default)]
    secret: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
}

#[derive(Deserialize)]
struct PatchBody {
    name: Option<String>,
    url: Option<String>,
    secret: Option<String>,
    enabled: Option<bool>,
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn read_router() -> Router<AppState> {
    Router::new()
        .route("/environments/{env_id}/webhooks", get(list_webhooks))
        .route(
            "/environments/{env_id}/webhooks/{id}/deliveries",
            get(list_deliveries),
        )
}

pub fn write_router() -> Router<AppState> {
    Router::new()
        .route("/environments/{env_id}/webhooks", post(create_webhook))
        .route(
            "/environments/{env_id}/webhooks/{id}",
            axum::routing::patch(patch_webhook).delete(delete_webhook),
        )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_webhooks(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(env_id): Path<String>,
) -> Result<Json<Vec<Webhook>>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    let rows = sqlx::query(
        "SELECT id::text, environment_id::text, name, url, \
         secret IS NOT NULL AS has_secret, enabled, created_at::text \
         FROM webhooks WHERE environment_id = $1::uuid ORDER BY created_at ASC",
    )
    .bind(&env_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to list webhooks");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let hooks = rows
        .iter()
        .map(|r| Webhook {
            id: r.get("id"),
            environment_id: r.get("environment_id"),
            name: r.get("name"),
            url: r.get("url"),
            has_secret: r.get("has_secret"),
            enabled: r.get("enabled"),
            created_at: r.get("created_at"),
        })
        .collect();

    Ok(Json(hooks))
}

async fn create_webhook(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(env_id): Path<String>,
    Json(body): Json<CreateBody>,
) -> Result<Json<Webhook>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    if body.url.is_empty() || crate::webhook_fire::refused_destination(&body.url) {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let row = sqlx::query(
        "INSERT INTO webhooks (environment_id, name, url, secret, enabled) \
         VALUES ($1::uuid, $2, $3, $4, $5) \
         RETURNING id::text, environment_id::text, name, url, \
                   secret IS NOT NULL AS has_secret, enabled, created_at::text",
    )
    .bind(&env_id)
    .bind(&body.name)
    .bind(&body.url)
    .bind(&body.secret)
    .bind(body.enabled)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to create webhook");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let hook = Webhook {
        id: row.get("id"),
        environment_id: row.get("environment_id"),
        name: row.get("name"),
        url: row.get("url"),
        has_secret: row.get("has_secret"),
        enabled: row.get("enabled"),
        created_at: row.get("created_at"),
    };

    info!(env_id = %env_id, webhook_id = %hook.id, "Webhook created");
    Ok(Json(hook))
}

async fn patch_webhook(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, id)): Path<(String, String)>,
    Json(body): Json<PatchBody>,
) -> Result<Json<Webhook>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;
    if body
        .url
        .as_deref()
        .is_some_and(crate::webhook_fire::refused_destination)
    {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let row = sqlx::query(
        "UPDATE webhooks SET \
         name    = COALESCE($1, name), \
         url     = COALESCE($2, url), \
         secret  = COALESCE($3, secret), \
         enabled = COALESCE($4, enabled) \
         WHERE id = $5::uuid AND environment_id = $6::uuid \
         RETURNING id::text, environment_id::text, name, url, \
                   secret IS NOT NULL AS has_secret, enabled, created_at::text",
    )
    .bind(&body.name)
    .bind(&body.url)
    .bind(&body.secret)
    .bind(body.enabled)
    .bind(&id)
    .bind(&env_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to update webhook");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(Webhook {
        id: row.get("id"),
        environment_id: row.get("environment_id"),
        name: row.get("name"),
        url: row.get("url"),
        has_secret: row.get("has_secret"),
        enabled: row.get("enabled"),
        created_at: row.get("created_at"),
    }))
}

async fn delete_webhook(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, id)): Path<(String, String)>,
) -> Result<StatusCode, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    let result =
        sqlx::query("DELETE FROM webhooks WHERE id = $1::uuid AND environment_id = $2::uuid")
            .bind(&id)
            .bind(&env_id)
            .execute(&state.db)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to delete webhook");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

    if result.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    info!(env_id = %env_id, webhook_id = %id, "Webhook deleted");
    Ok(StatusCode::NO_CONTENT)
}

async fn list_deliveries(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, id)): Path<(String, String)>,
) -> Result<Json<Vec<WebhookDelivery>>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    // Verify webhook belongs to this env before returning its delivery log.
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM webhooks WHERE id = $1::uuid AND environment_id = $2::uuid)",
    )
    .bind(&id)
    .bind(&env_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error verifying webhook ownership");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if !exists {
        return Err(StatusCode::NOT_FOUND);
    }

    let rows = sqlx::query(
        "SELECT id, webhook_id::text, event, status_code, response_body, error, delivered_at::text \
         FROM webhook_deliveries WHERE webhook_id = $1::uuid \
         ORDER BY delivered_at DESC LIMIT 100",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to list webhook deliveries");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let deliveries = rows
        .iter()
        .map(|r| WebhookDelivery {
            id: r.get("id"),
            webhook_id: r.get("webhook_id"),
            event: r.get("event"),
            status_code: r.get("status_code"),
            response_body: r.get("response_body"),
            error: r.get("error"),
            delivered_at: r.get("delivered_at"),
        })
        .collect();

    Ok(Json(deliveries))
}

// ---------------------------------------------------------------------------
// Helper: build the standard webhook payload
// ---------------------------------------------------------------------------

pub fn flag_event_payload(
    event: &str,
    env_id: &str,
    flag_key: &str,
    flag: Option<&Value>,
    actor_email: Option<&str>,
    metadata: Option<&Value>,
) -> Value {
    serde_json::json!({
        "event": event,
        "environment_id": env_id,
        "flag_key": flag_key,
        "flag": flag,
        "actor_email": actor_email,
        "metadata": metadata,
        "timestamp": chrono_or_now(),
    })
}

fn chrono_or_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Produce a minimal ISO-8601-like string without extra deps.
    format!("{secs}")
}
