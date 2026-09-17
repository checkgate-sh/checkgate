//! Personal access tokens — scoped, user-owned API credentials for CI/CD,
//! Terraform, and other automation. Unlike SDK keys (always admin-equivalent,
//! managed by an operator), a PAT acts *as its owning user* — same role, same
//! project memberships — and can be capped to `read_only`.
//!
//! Tokens are strictly self-service: a user can only list/create/revoke their
//! own tokens. There is no admin override — revoking access to a compromised
//! or departing account's automation is done by deleting the user (which
//! cascades to their tokens), matching how project membership already works.

use crate::auth::{AuthContext, get_session_claims, hash_token};
use crate::state::AppState;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use rand::RngExt as _;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tracing::{error, info};

fn format_rfc3339(dt: time::OffsetDateTime) -> String {
    dt.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| dt.to_string())
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Safe to return in list responses — does NOT include the token value.
#[derive(Serialize)]
pub struct TokenInfo {
    pub id: i64,
    pub name: String,
    /// First 12 chars of the token + "…" — safe to display.
    pub prefix: String,
    pub scope: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub expires_at: Option<String>,
}

/// Returned only once: when the token is first created.
#[derive(Serialize)]
pub struct NewTokenResponse {
    pub id: i64,
    pub name: String,
    /// Full token value — shown only once, never again.
    pub token: String,
    pub prefix: String,
    pub scope: String,
    pub created_at: String,
    pub expires_at: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateTokenRequest {
    pub name: String,
    /// `"read_only"` or `"read_write"`.
    pub scope: String,
    /// Number of days until expiry. `None` (or omitted) never expires.
    pub expires_in_days: Option<i64>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn generate_token() -> String {
    let mut bytes = [0u8; 24];
    rand::rng().fill(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("pat_{hex}")
}

fn prefix_of(token: &str) -> String {
    format!("{}…", token.chars().take(12).collect::<String>())
}

/// Resolves the calling user's id from their session/PAT identity. Bare SDK
/// key auth (no per-user identity) cannot own tokens — there's no "self" to
/// scope them to.
async fn current_user_id(db: &sqlx::PgPool, ctx: &AuthContext) -> Result<i64, StatusCode> {
    let claims = get_session_claims(ctx).ok_or(StatusCode::UNAUTHORIZED)?;
    sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE email = $1")
        .bind(&claims.email)
        .fetch_optional(db)
        .await
        .map_err(|e| {
            error!(error = %e, "DB error resolving caller's user id");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::UNAUTHORIZED)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub async fn list_tokens(
    State(state): State<AppState>,
    ctx: AuthContext,
) -> Result<Json<Vec<TokenInfo>>, StatusCode> {
    let user_id = current_user_id(&state.db, &ctx).await?;

    let rows = sqlx::query(
        "SELECT id, name, prefix, scope, created_at, last_used_at, expires_at \
         FROM personal_access_tokens WHERE user_id = $1 ORDER BY created_at ASC",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to list personal access tokens");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let tokens = rows
        .iter()
        .map(|row| TokenInfo {
            id: row.get("id"),
            name: row.get("name"),
            prefix: row.get("prefix"),
            scope: row.get("scope"),
            created_at: format_rfc3339(row.get("created_at")),
            last_used_at: row
                .get::<Option<time::OffsetDateTime>, _>("last_used_at")
                .map(format_rfc3339),
            expires_at: row
                .get::<Option<time::OffsetDateTime>, _>("expires_at")
                .map(format_rfc3339),
        })
        .collect();

    Ok(Json(tokens))
}

#[allow(
    clippy::arithmetic_side_effects,
    reason = "`days` is validated into 1..=365 by the match arm guard before it reaches the addition"
)]
pub async fn create_token(
    State(state): State<AppState>,
    ctx: AuthContext,
    Json(req): Json<CreateTokenRequest>,
) -> Result<Json<NewTokenResponse>, StatusCode> {
    let user_id = current_user_id(&state.db, &ctx).await?;

    let name = req.name.trim().to_string();
    if name.is_empty() || name.len() > 100 {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    if !matches!(req.scope.as_str(), "read_only" | "read_write") {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    // A read_only PAT must not be able to mint a read_write PAT for the same
    // user — that would let a leaked read-only credential self-escalate.
    // Session cookies and read_write PATs are uncapped.
    if crate::auth::pat_scope(&ctx) == Some("read_only") && req.scope == "read_write" {
        return Err(StatusCode::FORBIDDEN);
    }
    let expires_at = match req.expires_in_days {
        None => None,
        Some(days) if (1..=365).contains(&days) => {
            Some(time::OffsetDateTime::now_utc() + time::Duration::days(days))
        }
        Some(_) => return Err(StatusCode::UNPROCESSABLE_ENTITY),
    };

    let token = generate_token();
    let hash = hash_token(&token);
    let prefix = prefix_of(&token);

    let row = sqlx::query(
        "INSERT INTO personal_access_tokens (user_id, name, token_hash, prefix, scope, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id, created_at",
    )
    .bind(user_id)
    .bind(&name)
    .bind(&hash)
    .bind(&prefix)
    .bind(&req.scope)
    .bind(expires_at)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to create personal access token");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let id: i64 = row.get("id");
    let created_at: time::OffsetDateTime = row.get("created_at");

    info!(token_id = id, user_id = user_id, scope = %req.scope, "Personal access token created");

    Ok(Json(NewTokenResponse {
        id,
        name,
        token,
        prefix,
        scope: req.scope,
        created_at: format_rfc3339(created_at),
        expires_at: expires_at.map(format_rfc3339),
    }))
}

pub async fn revoke_token(
    State(state): State<AppState>,
    ctx: AuthContext,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let user_id = current_user_id(&state.db, &ctx).await?;

    let result = sqlx::query("DELETE FROM personal_access_tokens WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(&state.db)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to revoke personal access token");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if result.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    info!(
        token_id = id,
        user_id = user_id,
        "Personal access token revoked"
    );
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Routers
// ---------------------------------------------------------------------------

/// Read-only — any authenticated user (self-scoped inside the handler).
pub fn read_router() -> Router<AppState> {
    Router::new().route("/tokens", get(list_tokens))
}

/// Create/revoke — self-service, no elevated role required (self-scoped
/// inside the handler; a viewer can create their own read-only token).
pub fn self_service_router() -> Router<AppState> {
    Router::new()
        .route("/tokens", post(create_token))
        .route("/tokens/{id}", delete(revoke_token))
}
