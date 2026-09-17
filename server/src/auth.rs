use crate::state::AppState;
use crate::state::SdkKeyEntry;
use axum::{
    extract::{FromRequestParts, State},
    http::{Request, StatusCode, request::Parts},
    middleware::Next,
    response::Response,
};
use axum_extra::extract::cookie::PrivateCookieJar;
use constant_time_eq::constant_time_eq;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::convert::Infallible;
use tracing::warn;

/// Full session claims — used by handlers that need the caller's email/role.
#[derive(Deserialize, Clone)]
pub(crate) struct SessionClaims {
    pub user_id: i64,
    pub expires_at: i64,
    pub email: String,
    #[allow(dead_code)]
    pub name: String,
    pub role: String,
}

/// Identity resolved from a validated personal access token — inserted into
/// request extensions by `require_auth` so `require_editor`/`require_admin`
/// (and, via [`AuthContext`], handler-level project-membership checks) can see
/// it without a second database round-trip.
///
/// Unlike an SDK key (always admin-equivalent), a PAT acts *as its owning
/// user* — same role, same project memberships — capped by `scope`.
#[derive(Clone)]
pub(crate) struct PatIdentity {
    pub claims: SessionClaims,
    /// `"read_only"` or `"read_write"`.
    pub scope: String,
}

/// Extract the SDK key from a request — Bearer header takes priority over the
/// `?sdk_key=` query param fallback used by browser EventSource clients.
/// Returns `None` if neither is present.
fn extract_sdk_key<'a>(headers: &'a axum::http::HeaderMap, query: &'a str) -> Option<&'a str> {
    if let Some(bearer) = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        return Some(bearer);
    }
    query.split('&').find_map(|p| p.strip_prefix("sdk_key="))
}

/// Extract a bearer token — unlike [`extract_sdk_key`], PATs are never
/// accepted via `?sdk_key=` query param (that fallback exists only for
/// SSE/EventSource, which PATs — automation credentials that can always set
/// headers — have no need of). Keeping PATs header-only avoids the exact
/// access-log/proxy-log exposure risk already known to affect SDK keys.
fn extract_bearer(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

pub(crate) fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Looks up a bearer token against `personal_access_tokens`. Returns `Ok(None)`
/// if the token doesn't match any live (non-expired) token — a normal "not a
/// PAT" outcome, not an error. Returns `Err` only on genuine DB failure.
async fn resolve_pat(db: &sqlx::PgPool, token: &str) -> Result<Option<PatIdentity>, ()> {
    let hash = hash_token(token);
    let row = sqlx::query(
        "SELECT pat.id, pat.scope, u.id AS user_id, u.email, u.name, u.role \
         FROM personal_access_tokens pat JOIN users u ON u.id = pat.user_id \
         WHERE pat.token_hash = $1 AND (pat.expires_at IS NULL OR pat.expires_at > NOW())",
    )
    .bind(&hash)
    .fetch_optional(db)
    .await
    .map_err(|e| {
        warn!(error = %e, "DB error resolving personal access token");
    })?;

    let Some(row) = row else {
        return Ok(None);
    };

    let id: i64 = row.get("id");
    // Best-effort — a failed timestamp update must never block the request.
    let _ = sqlx::query("UPDATE personal_access_tokens SET last_used_at = NOW() WHERE id = $1")
        .bind(id)
        .execute(db)
        .await;

    Ok(Some(PatIdentity {
        claims: SessionClaims {
            user_id: row.get("user_id"),
            expires_at: i64::MAX,
            email: row.get("email"),
            name: row.get("name"),
            role: row.get("role"),
        },
        scope: row.get("scope"),
    }))
}

/// Validate expiry and resolve the account's current identity, including its role.
pub(crate) async fn resolve_session(
    db: &sqlx::PgPool,
    jar: &PrivateCookieJar,
) -> Result<Option<SessionClaims>, StatusCode> {
    let Some(mut claims) = jar
        .get("lg_session")
        .and_then(|c| serde_json::from_str::<SessionClaims>(c.value()).ok())
    else {
        return Ok(None);
    };
    if claims.expires_at <= time::OffsetDateTime::now_utc().unix_timestamp() {
        return Ok(None);
    }
    let row = sqlx::query("SELECT email, name, role FROM users WHERE id = $1")
        .bind(claims.user_id)
        .fetch_optional(db)
        .await
        .map_err(|e| {
            warn!(error = %e, "Session lookup failed");
            StatusCode::SERVICE_UNAVAILABLE
        })?;
    let Some(row) = row else {
        return Ok(None);
    };
    claims.email = row.get("email");
    claims.name = row.get("name");
    claims.role = row.get("role");
    Ok(Some(claims))
}

/// Per-request user identities already validated by require_auth.
#[derive(Clone)]
pub(crate) struct AuthContext {
    session: Option<SessionClaims>,
    pat: Option<PatIdentity>,
}

impl<S> FromRequestParts<S> for AuthContext
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let session = parts.extensions.get::<SessionClaims>().cloned();
        let pat = parts.extensions.get::<PatIdentity>().cloned();
        Ok(AuthContext { session, pat })
    }
}

/// Resolves the caller's identity: session cookie takes priority (matches the
/// existing `require_auth` precedence), falling back to a PAT identity stashed
/// into request extensions by `require_auth`. Returns `None` for SDK-key auth
/// (no per-user identity — treated as admin-equivalent by callers, unchanged).
pub(crate) fn get_session_claims(ctx: &AuthContext) -> Option<SessionClaims> {
    ctx.session
        .clone()
        .or_else(|| ctx.pat.clone().map(|p| p.claims))
}

/// Returns the scope of the personal access token that authenticated this
/// request, or `None` if the caller used a session cookie or SDK key instead
/// (both uncapped — a session already carries full user identity, and an SDK
/// key is a separate, admin-equivalent credential class).
///
/// Used to stop a `read_only` PAT from minting a `read_write` PAT for the
/// same user — without this cap, a leaked read-only token could self-escalate
/// by simply creating a more privileged replacement.
pub(crate) fn pat_scope(ctx: &AuthContext) -> Option<&str> {
    if ctx.session.is_some() {
        return None;
    }
    ctx.pat.as_ref().map(|p| p.scope.as_str())
}

/// Validates a request using either:
///
/// 1. **Session cookie** (`lg_session`) — set by `POST /api/auth/login`.
///    Used by the dashboard SPA. The cookie is HttpOnly + encrypted.
///
/// 2. **Bearer token** (`Authorization: Bearer <key>`) — for SDK clients.
///
/// 3. **`?sdk_key=` query param** — fallback for browser `EventSource` which
///    cannot set custom headers. Exposes the key in access/proxy logs; only
///    use for SSE where headers cannot be set.
///
/// Returns 503 if authoritative credential lookup is unavailable.
/// Returns 401 if credentials are absent or invalid.
pub async fn require_auth(
    State(state): State<AppState>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let jar = PrivateCookieJar::from_headers(req.headers(), state.session_key.clone());
    if let Some(claims) = resolve_session(&state.db, &jar).await? {
        req.extensions_mut().insert(claims);
        return Ok(next.run(req).await);
    }

    let query = req.uri().query().unwrap_or("");
    if let Some(key) = extract_sdk_key(req.headers(), query) {
        // Database validation makes creation and revocation effective on every replica.
        let row = sqlx::query(
            "SELECT id, name, value, environment_id::text FROM sdk_keys WHERE value = $1",
        )
        .bind(key)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| {
            warn!(error = %e, "SDK key lookup failed");
            StatusCode::SERVICE_UNAVAILABLE
        })?;
        let entry = row
            .map(|row| SdkKeyEntry {
                id: row.get("id"),
                name: row.get("name"),
                value: row.get("value"),
                environment_id: Some(row.get("environment_id")),
            })
            .or_else(|| {
                std::env::var("SDK_KEY")
                    .ok()
                    .filter(|expected| {
                        !expected.is_empty()
                            && constant_time_eq(key.as_bytes(), expected.as_bytes())
                    })
                    .map(|value| SdkKeyEntry {
                        id: -1,
                        name: "env:SDK_KEY".into(),
                        value,
                        environment_id: None,
                    })
            });
        if let Some(entry) = entry {
            req.extensions_mut().insert(entry);
            return Ok(next.run(req).await);
        }
    }
    if let Some(token) = extract_bearer(req.headers())
        && let Some(pat) = resolve_pat(&state.db, token)
            .await
            .map_err(|()| StatusCode::SERVICE_UNAVAILABLE)?
    {
        req.extensions_mut().insert(pat);
        return Ok(next.run(req).await);
    }

    warn!(
        method = %req.method(),
        path = %req.uri().path(),
        "Rejected request: missing or invalid credentials"
    );

    Err(StatusCode::UNAUTHORIZED)
}

/// Role checks consume only identities validated by require_auth.
pub async fn require_admin(
    State(_state): State<AppState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if req.extensions().get::<SdkKeyEntry>().is_some() {
        return Ok(next.run(req).await);
    }
    let claims = request_claims(&req)?;
    if claims.role != "admin" {
        return Err(StatusCode::FORBIDDEN);
    }
    check_write_scope(&req)?;
    Ok(next.run(req).await)
}

fn request_claims(req: &Request<axum::body::Body>) -> Result<&SessionClaims, StatusCode> {
    req.extensions()
        .get::<SessionClaims>()
        .or_else(|| req.extensions().get::<PatIdentity>().map(|p| &p.claims))
        .ok_or(StatusCode::UNAUTHORIZED)
}

fn check_write_scope(req: &Request<axum::body::Body>) -> Result<(), StatusCode> {
    if req
        .extensions()
        .get::<PatIdentity>()
        .is_some_and(|p| p.scope != "read_write")
    {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

pub(crate) async fn check_env_role(
    db: &sqlx::PgPool,
    claims: &SessionClaims,
    env_id: &str,
    write: bool,
) -> Result<(), StatusCode> {
    if claims.role == "admin" {
        return Ok(());
    }
    let role: Option<String> = sqlx::query_scalar(
        "SELECT pm.role FROM project_members pm JOIN environments e ON e.project_id = pm.project_id \
         WHERE e.id = $1::uuid AND pm.user_id = $2",
    ).bind(env_id).bind(claims.user_id).fetch_optional(db).await
        .map_err(|e| { warn!(error = %e, "Project role lookup failed"); StatusCode::INTERNAL_SERVER_ERROR })?;
    match role.as_deref() {
        Some("admin" | "editor") => Ok(()),
        Some("viewer") if !write => Ok(()),
        _ => Err(StatusCode::FORBIDDEN),
    }
}

/// Scoped writes use the project membership role; workspace admins bypass it.
pub async fn require_editor(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if req.extensions().get::<SdkKeyEntry>().is_some() {
        return Ok(next.run(req).await);
    }
    check_write_scope(&req)?;
    let claims = request_claims(&req)?;
    let mut parts = req.uri().path().split('/');
    if let Some(env_id) = parts
        .find(|p| *p == "environments")
        .and_then(|_| parts.next())
    {
        check_env_role(&state.db, claims, env_id, true).await?;
    } else if !matches!(claims.role.as_str(), "admin" | "editor") {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(next.run(req).await)
}
