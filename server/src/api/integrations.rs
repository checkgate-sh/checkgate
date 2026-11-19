//! CRUD for chat integrations (Slack / Microsoft Teams).
//!
//! Mirrors [`super::webhooks`] — same env-scoped access check and shape — but
//! the stored row targets a provider incoming-webhook URL and carries an event
//! subscription filter. Rendering and delivery live in
//! [`crate::integrations`].

use crate::auth::AuthContext;
use crate::state::AppState;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tracing::{error, info};

use super::flags::check_env_access;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct Integration {
    pub id: String,
    pub environment_id: String,
    pub kind: String,
    pub name: String,
    /// The incoming-webhook URL is a bearer secret — anyone holding it can post
    /// to the channel — so it is never returned after creation.
    pub webhook_url_preview: String,
    pub events: Vec<String>,
    pub enabled: bool,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct IntegrationDelivery {
    pub id: i64,
    pub integration_id: String,
    pub event: String,
    pub status_code: Option<i32>,
    pub response_body: Option<String>,
    pub error: Option<String>,
    pub delivered_at: String,
}

#[derive(Deserialize)]
struct CreateBody {
    kind: String,
    name: String,
    webhook_url: String,
    #[serde(default)]
    events: Vec<String>,
    #[serde(default = "default_true")]
    enabled: bool,
}

#[derive(Deserialize)]
struct PatchBody {
    name: Option<String>,
    webhook_url: Option<String>,
    events: Option<Vec<String>>,
    enabled: Option<bool>,
}

fn default_true() -> bool {
    true
}

/// Show enough of the URL to tell two integrations apart, without handing back
/// a credential that would let the holder post into the channel.
fn preview_url(url: &str) -> String {
    // Provider URLs end in a high-entropy path segment; the tail is the part a
    // human recognises, and 6 chars is far too few to replay.
    let tail: String = url
        .chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

fn row_to_integration(r: &sqlx::postgres::PgRow) -> Integration {
    Integration {
        id: r.get("id"),
        environment_id: r.get("environment_id"),
        kind: r.get("kind"),
        name: r.get("name"),
        webhook_url_preview: preview_url(r.get::<String, _>("webhook_url").as_str()),
        events: r.get("events"),
        enabled: r.get("enabled"),
        created_at: r.get("created_at"),
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

const VALID_KINDS: &[&str] = &["slack", "teams"];

/// Webhook URLs are bearer credentials, so plaintext is refused — except on
/// loopback, which never leaves the machine and is what self-hosted operators
/// point at a local relay (and what the test suite drives).
///
/// The host is compared exactly against the loopback set after stripping any
/// port, so lookalikes like `http://127.0.0.1.evil.com/` are rejected: their
/// host is `127.0.0.1.evil.com`, which matches nothing here.
fn is_allowed_url(url: &str) -> bool {
    if crate::webhook_fire::refused_destination(url) {
        return false;
    }
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    // Authority is everything before the first '/', '?' or '#'.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // Credentials in the authority (`user@host`) would move the real host after
    // the '@' — take that side.
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let host = match host_port.strip_prefix('[') {
        // IPv6 literal: `[::1]:8080`
        Some(after) => after.split(']').next().unwrap_or_default(),
        None => host_port.split(':').next().unwrap_or_default(),
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

/// Rejects unknown providers and unknown event names before they reach the DB.
/// The `kind` CHECK constraint would catch the former as a 500; this turns it
/// into an actionable 422, and catches typo'd event names the DB can't see.
fn validate(kind: &str, url: &str, events: &[String]) -> Result<(), StatusCode> {
    if !VALID_KINDS.contains(&kind) {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    if url.is_empty() || !is_allowed_url(url) {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    for e in events {
        if !crate::integrations::SUPPORTED_EVENTS.contains(&e.as_str()) {
            return Err(StatusCode::UNPROCESSABLE_ENTITY);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn read_router() -> Router<AppState> {
    Router::new()
        .route(
            "/environments/{env_id}/integrations",
            get(list_integrations),
        )
        .route(
            "/environments/{env_id}/integrations/{id}/deliveries",
            get(list_deliveries),
        )
}

pub fn write_router() -> Router<AppState> {
    Router::new()
        .route(
            "/environments/{env_id}/integrations",
            post(create_integration),
        )
        .route(
            "/environments/{env_id}/integrations/{id}",
            axum::routing::patch(patch_integration).delete(delete_integration),
        )
        .route(
            "/environments/{env_id}/integrations/{id}/test",
            post(test_integration),
        )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_integrations(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(env_id): Path<String>,
) -> Result<Json<Vec<Integration>>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    let rows = sqlx::query(
        "SELECT id::text, environment_id::text, kind, name, webhook_url, events, enabled, created_at::text \
         FROM integrations WHERE environment_id = $1::uuid ORDER BY created_at ASC",
    )
    .bind(&env_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to list integrations");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(rows.iter().map(row_to_integration).collect()))
}

async fn create_integration(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(env_id): Path<String>,
    Json(body): Json<CreateBody>,
) -> Result<Json<Integration>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;
    validate(&body.kind, &body.webhook_url, &body.events)?;

    let row = sqlx::query(
        "INSERT INTO integrations (environment_id, kind, name, webhook_url, events, enabled) \
         VALUES ($1::uuid, $2, $3, $4, $5, $6) \
         RETURNING id::text, environment_id::text, kind, name, webhook_url, events, enabled, created_at::text",
    )
    .bind(&env_id)
    .bind(&body.kind)
    .bind(&body.name)
    .bind(&body.webhook_url)
    .bind(&body.events)
    .bind(body.enabled)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to create integration");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let integration = row_to_integration(&row);
    info!(env_id = %env_id, integration_id = %integration.id, kind = %integration.kind, "Integration created");
    Ok(Json(integration))
}

async fn patch_integration(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, id)): Path<(String, String)>,
    Json(body): Json<PatchBody>,
) -> Result<Json<Integration>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    if let Some(url) = &body.webhook_url
        && (url.is_empty() || !is_allowed_url(url))
    {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    if let Some(events) = &body.events {
        for e in events {
            if !crate::integrations::SUPPORTED_EVENTS.contains(&e.as_str()) {
                return Err(StatusCode::UNPROCESSABLE_ENTITY);
            }
        }
    }

    let row = sqlx::query(
        "UPDATE integrations SET \
         name        = COALESCE($1, name), \
         webhook_url = COALESCE($2, webhook_url), \
         events      = COALESCE($3, events), \
         enabled     = COALESCE($4, enabled) \
         WHERE id = $5::uuid AND environment_id = $6::uuid \
         RETURNING id::text, environment_id::text, kind, name, webhook_url, events, enabled, created_at::text",
    )
    .bind(&body.name)
    .bind(&body.webhook_url)
    .bind(&body.events)
    .bind(body.enabled)
    .bind(&id)
    .bind(&env_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to update integration");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

    info!(env_id = %env_id, integration_id = %id, "Integration updated");
    Ok(Json(row_to_integration(&row)))
}

async fn delete_integration(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, id)): Path<(String, String)>,
) -> Result<StatusCode, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    let result =
        sqlx::query("DELETE FROM integrations WHERE id = $1::uuid AND environment_id = $2::uuid")
            .bind(&id)
            .bind(&env_id)
            .execute(&state.db)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to delete integration");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

    if result.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    info!(env_id = %env_id, integration_id = %id, "Integration deleted");
    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/environments/:env_id/integrations/:id/test — send a sample message
/// so the channel wiring can be confirmed before a real event depends on it.
async fn test_integration(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, id)): Path<(String, String)>,
) -> Result<StatusCode, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    let exists = sqlx::query(
        "SELECT 1 AS ok FROM integrations WHERE id = $1::uuid AND environment_id = $2::uuid",
    )
    .bind(&id)
    .bind(&env_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to look up integration for test");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if exists.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    // Reuses the normal delivery path, so a passing test exercises exactly what
    // a real event will do — including the subscription filter.
    let payload = super::webhooks::flag_event_payload(
        "flag.updated",
        &env_id,
        "checkgate_test_flag",
        Some(&serde_json::json!({ "is_enabled": true, "rollout_percentage": 100 })),
        Some("checkgate"),
        None,
    );
    crate::integrations::fire_integrations(state.clone(), env_id.clone(), payload);

    info!(env_id = %env_id, integration_id = %id, "Integration test message queued");
    Ok(StatusCode::ACCEPTED)
}

async fn list_deliveries(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, id)): Path<(String, String)>,
) -> Result<Json<Vec<IntegrationDelivery>>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    let rows = sqlx::query(
        "SELECT d.id, d.integration_id::text, d.event, d.status_code, d.response_body, \
                d.error, d.delivered_at::text \
         FROM integration_deliveries d \
         JOIN integrations i ON i.id = d.integration_id \
         WHERE d.integration_id = $1::uuid AND i.environment_id = $2::uuid \
         ORDER BY d.delivered_at DESC LIMIT 50",
    )
    .bind(&id)
    .bind(&env_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to list integration deliveries");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let deliveries = rows
        .iter()
        .map(|r| IntegrationDelivery {
            id: r.get("id"),
            integration_id: r.get("integration_id"),
            event: r.get("event"),
            status_code: r.get("status_code"),
            response_body: r.get("response_body"),
            error: r.get("error"),
            delivered_at: r.get("delivered_at"),
        })
        .collect();

    Ok(Json(deliveries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_hides_all_but_the_tail() {
        let url = "https://hooks.slack.com/services/T000/B000/XXXXsecretXXXX";
        let preview = preview_url(url);
        assert_eq!(preview, "…etXXXX");
        assert!(!preview.contains("hooks.slack.com"));
        assert!(!preview.contains("secret"));
        // Whatever the tail length, the credential-bearing path must not survive.
        assert!(preview.len() < url.len() / 4);
    }

    #[test]
    fn preview_handles_short_urls_without_panicking() {
        assert_eq!(preview_url("abc"), "…abc");
        assert_eq!(preview_url(""), "…");
    }

    #[test]
    fn rejects_unknown_provider() {
        assert_eq!(
            validate("irc", "https://example.com/hook", &[]),
            Err(StatusCode::UNPROCESSABLE_ENTITY)
        );
    }

    #[test]
    fn requires_https_off_loopback() {
        // These URLs are bearer credentials; posting them over plaintext would
        // leak the ability to write into the channel.
        assert_eq!(
            validate("slack", "http://hooks.slack.com/x", &[]),
            Err(StatusCode::UNPROCESSABLE_ENTITY)
        );
        assert!(validate("slack", "https://hooks.slack.com/x", &[]).is_ok());
    }

    #[test]
    fn allows_plaintext_only_on_loopback() {
        assert!(is_allowed_url("http://127.0.0.1:8080/hook"));
        assert!(is_allowed_url("http://localhost/hook"));
        assert!(is_allowed_url("http://[::1]:3000/hook"));
        assert!(is_allowed_url("https://anything.example.com/hook"));
    }

    #[test]
    fn loopback_lookalikes_are_rejected() {
        // Each of these would resolve to an attacker-controlled host while
        // *looking* like loopback to a naive prefix/contains check.
        for url in [
            "http://127.0.0.1.evil.com/hook",
            "http://localhost.evil.com/hook",
            "http://evil.com/127.0.0.1",
            "http://evil.com#127.0.0.1",
            "http://evil.com?x=127.0.0.1",
            // Credentials in the authority put the real host after '@'.
            "http://127.0.0.1@evil.com/hook",
            "http://user:pass@evil.com/hook",
        ] {
            assert!(!is_allowed_url(url), "should reject {url}");
        }
    }

    #[test]
    fn non_http_schemes_are_rejected() {
        assert!(!is_allowed_url("ftp://127.0.0.1/hook"));
        assert!(!is_allowed_url("file:///etc/passwd"));
        assert!(!is_allowed_url("127.0.0.1/hook"));
        assert!(!is_allowed_url(""));
    }

    #[test]
    fn rejects_unknown_event_names() {
        let events = vec!["flag.exploded".to_string()];
        assert_eq!(
            validate("slack", "https://hooks.slack.com/x", &events),
            Err(StatusCode::UNPROCESSABLE_ENTITY)
        );
    }

    #[test]
    fn accepts_every_supported_event() {
        let events: Vec<String> = crate::integrations::SUPPORTED_EVENTS
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(validate("teams", "https://outlook.office.com/webhook/x", &events).is_ok());
    }
}
