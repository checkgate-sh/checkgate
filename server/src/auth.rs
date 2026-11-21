use crate::state::AppState;
use crate::state::SdkKeyEntry;
use axum::{
    extract::{FromRequestParts, State},
    http::{Method, Request, StatusCode, request::Parts},
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
/// Unlike an SDK key (limited to the SDK routes of its own environment — see
/// [`sdk_key_route_allowed`]), a PAT acts *as its owning user* — same role,
/// same project memberships — capped by `scope`.
#[derive(Clone)]
pub(crate) struct PatIdentity {
    pub claims: SessionClaims,
    /// [`SCOPE_READ_ONLY`], [`SCOPE_PROPOSE`] or [`SCOPE_READ_WRITE`]. Any
    /// other value (a scope added by a newer release, then rolled back) is
    /// treated as read-only: only `read_write` passes the write checks, and
    /// only `propose` reaches the [`propose_route_allowed`] routes.
    pub scope: String,
    /// When the token expires; `None` for a token that never does. A token
    /// created with this one may not outlive it.
    pub expires_at: Option<time::OffsetDateTime>,
}

/// Token scope: reads only.
pub(crate) const SCOPE_READ_ONLY: &str = "read_only";
/// Token scope: reads, plus *proposing* flag changes that a different person
/// must approve. Meant for AI agents and other automation that should never
/// apply, approve or reject a change on its own. See [`propose_route_allowed`].
pub(crate) const SCOPE_PROPOSE: &str = "propose";
/// Token scope: everything the owning user may do.
pub(crate) const SCOPE_READ_WRITE: &str = "read_write";

/// Scopes `POST /api/tokens` accepts.
pub(crate) const KNOWN_SCOPES: [&str; 3] = [SCOPE_READ_ONLY, SCOPE_PROPOSE, SCOPE_READ_WRITE];

/// How capable a scope is. The scopes form a chain: every route a
/// `read_only` token may call, a `propose` token may too, and a `read_write`
/// token can do at least what a `propose` token can (change a flag —
/// directly where no approval is required — and withdraw its own request). A token may mint another only when the new one ranks no higher.
/// An unknown scope ranks with `read_only` (fail closed).
pub(crate) fn scope_rank(scope: &str) -> u8 {
    match scope {
        SCOPE_READ_WRITE => 2,
        SCOPE_PROPOSE => 1,
        _ => 0,
    }
}

/// The only write routes a `propose` token may reach — each handler then
/// narrows what it does for such a token:
///
/// - `PATCH /api/environments/{env}/flags/{key}` always queues a pending
///   change request (`202`), whether or not the environment requires approval;
/// - `DELETE /api/environments/{env}/change-requests/{id}` only withdraws the
///   token owner's own pending request (no admin override).
///
/// Anything else that writes — creating a flag, approve, reject, delete,
/// promote, archive, segments, environments, keys, users, webhooks, tokens —
/// is `403`. Creating is excluded because a new flag is served to SDKs at
/// once: even a disabled one makes code that already reads the key get
/// `false` instead of its own default, with no review.
///
/// `path` may still carry the `/api` prefix (it does in `require_auth`, not
/// inside the nested router). Segments are compared exactly, without
/// dropping empty ones, so a path this accepts has the shape of exactly one
/// route.
fn propose_route_allowed(method: &Method, path: &str) -> bool {
    let trimmed = path.strip_prefix('/').unwrap_or(path);
    let segments: Vec<&str> = trimmed.split('/').collect();
    let rest = match segments.as_slice() {
        ["api", rest @ ..] => rest,
        rest => rest,
    };
    let param = |s: &str| !s.is_empty();
    match (method, rest) {
        (&Method::PATCH, ["environments", env, "flags", key]) => param(env) && param(key),
        (&Method::DELETE, ["environments", env, "change-requests", id]) => param(env) && param(id),
        _ => false,
    }
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
        "SELECT pat.id, pat.scope, pat.expires_at, u.id AS user_id, u.email, u.name, u.role \
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
        expires_at: row.get("expires_at"),
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

/// Per-request identities already validated by require_auth.
#[derive(Clone)]
pub(crate) struct AuthContext {
    session: Option<SessionClaims>,
    pat: Option<PatIdentity>,
    sdk_key: Option<SdkKeyEntry>,
}

impl<S> FromRequestParts<S> for AuthContext
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let session = parts.extensions.get::<SessionClaims>().cloned();
        let pat = parts.extensions.get::<PatIdentity>().cloned();
        let sdk_key = parts.extensions.get::<SdkKeyEntry>().cloned();
        Ok(AuthContext {
            session,
            pat,
            sdk_key,
        })
    }
}

/// Resolves the caller's identity: session cookie takes priority (matches the
/// existing `require_auth` precedence), falling back to a PAT identity stashed
/// into request extensions by `require_auth`. Returns `None` for SDK-key auth,
/// which has no per-user identity. SDK keys only ever reach the routes listed
/// in [`sdk_key_route_allowed`], so a `None` here never grants admin rights.
pub(crate) fn get_session_claims(ctx: &AuthContext) -> Option<SessionClaims> {
    ctx.session
        .clone()
        .or_else(|| ctx.pat.clone().map(|p| p.claims))
}

/// Returns the scope of the personal access token that authenticated this
/// request, or `None` if the caller used a session cookie or SDK key instead
/// (a session already carries full user identity, and an SDK key is confined
/// to the SDK routes by `require_auth`).
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

/// True when the request was authenticated by a `propose`-scoped personal
/// access token. Handlers on the [`propose_route_allowed`] routes use this to
/// narrow their behaviour; a session or another scope returns false.
pub(crate) fn is_propose_token(ctx: &AuthContext) -> bool {
    pat_scope(ctx) == Some(SCOPE_PROPOSE)
}

/// For a request authenticated by a personal access token, that token's
/// expiry (`Some(None)` if it never expires); `None` for a session or SDK key.
pub(crate) fn pat_expires_at(ctx: &AuthContext) -> Option<Option<time::OffsetDateTime>> {
    if ctx.session.is_some() {
        return None;
    }
    ctx.pat.as_ref().map(|p| p.expires_at)
}

/// The SDK key that authenticated this request, if any.
pub(crate) fn sdk_key(ctx: &AuthContext) -> Option<&SdkKeyEntry> {
    ctx.sdk_key.as_ref()
}

/// SDK keys ship inside browser and mobile bundles, so they must be treated as
/// public. They may only use what an SDK needs, and only for the environment
/// they are bound to: the SSE stream, the poll snapshot, reading flags (used by
/// `checkgate typegen`), and reporting impressions/events. The legacy
/// `SDK_KEY` env var has no bound environment and may target any one.
fn sdk_key_route_allowed(method: &Method, path: &str, key_env: Option<&str>) -> bool {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let env_ok = |env: &str| key_env.is_none_or(|bound| bound == env);
    match (method, segments.as_slice()) {
        (&Method::GET, ["stream"] | ["flags", "snapshot"]) => true,
        (&Method::GET, ["api", "environments", env, "flags"])
        | (&Method::GET, ["api", "environments", env, "flags", _])
        | (&Method::POST, ["api", "environments", env, "impressions" | "events"]) => env_ok(env),
        _ => false,
    }
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
            if !sdk_key_route_allowed(
                req.method(),
                req.uri().path(),
                entry.environment_id.as_deref(),
            ) {
                warn!(
                    method = %req.method(),
                    path = %req.uri().path(),
                    sdk_key = %entry.name,
                    "Rejected SDK key outside its SDK routes"
                );
                return Err(StatusCode::FORBIDDEN);
            }
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
        return Err(StatusCode::FORBIDDEN);
    }
    let claims = request_claims(&req)?;
    if claims.role != "admin" {
        return Err(StatusCode::FORBIDDEN);
    }
    // Admin routes are never open to a `propose` token.
    check_write_scope(&req, false)?;
    Ok(next.run(req).await)
}

fn request_claims(req: &Request<axum::body::Body>) -> Result<&SessionClaims, StatusCode> {
    req.extensions()
        .get::<SessionClaims>()
        .or_else(|| req.extensions().get::<PatIdentity>().map(|p| &p.claims))
        .ok_or(StatusCode::UNAUTHORIZED)
}

/// Handler-level write check: only a `read_write` token (or a session)
/// passes. `propose` and any unknown scope are refused.
pub(crate) fn check_write_scope_ctx(ctx: &AuthContext) -> Result<(), StatusCode> {
    if pat_scope(ctx).is_some_and(|scope| scope != SCOPE_READ_WRITE) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// Middleware-level write check. A session or `read_write` token passes; a
/// `propose` token passes only when `allow_propose` is set and the request is
/// one of the [`propose_route_allowed`] routes; every other scope — including
/// one this build does not know — is refused.
fn check_write_scope(
    req: &Request<axum::body::Body>,
    allow_propose: bool,
) -> Result<(), StatusCode> {
    let Some(pat) = req.extensions().get::<PatIdentity>() else {
        return Ok(());
    };
    let allowed = pat.scope == SCOPE_READ_WRITE
        || (allow_propose
            && pat.scope == SCOPE_PROPOSE
            && propose_route_allowed(req.method(), req.uri().path()));
    if allowed {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
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
        return Err(StatusCode::FORBIDDEN);
    }
    check_write_scope(&req, true)?;
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

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;

    #[test]
    fn propose_routes_are_exactly_two() {
        for (method, path) in [
            (Method::PATCH, "/api/environments/e1/flags/new-ui"),
            (Method::PATCH, "/environments/e1/flags/new-ui"),
            (Method::DELETE, "/api/environments/e1/change-requests/7"),
            (Method::DELETE, "/environments/e1/change-requests/7"),
        ] {
            assert!(propose_route_allowed(&method, path), "{method} {path}");
        }
        for (method, path) in [
            // Creating a flag serves it to SDKs at once (F-01).
            (Method::POST, "/api/environments/e1/flags"),
            (Method::POST, "/environments/e1/flags"),
            (Method::DELETE, "/api/environments/e1/flags/new-ui"),
            (Method::PUT, "/api/environments/e1/flags/new-ui"),
            (Method::POST, "/api/environments/e1/flags/new-ui/promote"),
            (Method::POST, "/api/environments/e1/flags/new-ui/archive"),
            (Method::POST, "/api/environments/e1/flags/new-ui/unarchive"),
            (
                Method::POST,
                "/api/environments/e1/change-requests/7/approve",
            ),
            (
                Method::POST,
                "/api/environments/e1/change-requests/7/reject",
            ),
            (Method::POST, "/api/environments/e1/segments"),
            (Method::PATCH, "/api/environments/e1/segments/beta"),
            (Method::POST, "/api/environments/e1/webhooks"),
            (
                Method::POST,
                "/api/environments/e1/flags/new-ui/scheduled-changes",
            ),
            (Method::POST, "/api/environments/e1/experiments"),
            (Method::POST, "/api/environments/e1/impressions"),
            (Method::POST, "/api/tokens"),
            (Method::DELETE, "/api/tokens/3"),
            (Method::POST, "/api/users"),
            (Method::POST, "/api/projects/p1/environments"),
            (Method::GET, "/api/environments/e1/flags"),
            // Shapes that only look similar.
            (Method::POST, "/api/environments//flags"),
            (Method::PATCH, "/api/environments/e1/flags/"),
            (Method::PATCH, "/api/environments/e1//flags/k"),
            (Method::PATCH, "/api/api/environments/e1/flags/k"),
            (Method::PATCH, "/api/environments/e1/flags/k/extra"),
            (Method::DELETE, "/api/environments/e1/change-requests/"),
        ] {
            assert!(!propose_route_allowed(&method, path), "{method} {path}");
        }
    }

    #[test]
    fn scope_rank_is_a_chain_and_unknown_is_lowest() {
        assert!(scope_rank(SCOPE_READ_ONLY) < scope_rank(SCOPE_PROPOSE));
        assert!(scope_rank(SCOPE_PROPOSE) < scope_rank(SCOPE_READ_WRITE));
        assert_eq!(scope_rank("admin"), scope_rank(SCOPE_READ_ONLY));
        assert_eq!(scope_rank(""), scope_rank(SCOPE_READ_ONLY));
        assert_eq!(scope_rank("READ_WRITE"), scope_rank(SCOPE_READ_ONLY));
    }

    fn pat_request(scope: &str, method: Method, path: &str) -> Request<axum::body::Body> {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .body(axum::body::Body::empty())
            .unwrap();
        req.extensions_mut().insert(PatIdentity {
            claims: SessionClaims {
                user_id: 1,
                expires_at: i64::MAX,
                email: "agent-owner@acme.test".into(),
                name: "Owner".into(),
                role: "admin".into(),
            },
            scope: scope.into(),
            expires_at: None,
        });
        req
    }

    #[test]
    fn write_scope_fails_closed() {
        let patch = |scope| pat_request(scope, Method::PATCH, "/environments/e1/flags/k");
        let promote = |scope| pat_request(scope, Method::POST, "/environments/e1/flags/k/promote");
        // read_write: everything.
        assert!(check_write_scope(&patch(SCOPE_READ_WRITE), true).is_ok());
        assert!(check_write_scope(&promote(SCOPE_READ_WRITE), false).is_ok());
        // propose: only its routes, and never where the caller disallows it.
        assert!(check_write_scope(&patch(SCOPE_PROPOSE), true).is_ok());
        assert!(check_write_scope(&patch(SCOPE_PROPOSE), false).is_err());
        assert!(check_write_scope(&promote(SCOPE_PROPOSE), true).is_err());
        // read_only and scopes this build does not know: nothing.
        for scope in [SCOPE_READ_ONLY, "future_scope", ""] {
            assert!(check_write_scope(&patch(scope), true).is_err(), "{scope}");
        }
        // No token at all (a session): the role checks decide.
        let session = Request::builder()
            .method(Method::PATCH)
            .uri("/environments/e1/flags/k")
            .body(axum::body::Body::empty())
            .unwrap();
        assert!(check_write_scope(&session, false).is_ok());
    }
}
