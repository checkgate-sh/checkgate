pub mod audit;
pub mod change_requests;
pub mod environments;
pub mod events;
pub mod experiments;
pub mod flags;
pub mod health;
pub mod impressions;
pub mod integrations;
pub mod keys;
pub mod projects;
pub mod scheduled;
pub mod segments;
pub mod session;
pub mod tokens;
pub mod users;
pub mod webhooks;

use crate::state::AppState;
use axum::{
    Router,
    body::Body,
    http::{HeaderValue, Method, Request, StatusCode, header},
    middleware::Next,
    response::Response,
    routing::{get, post},
};
use tracing::warn;

/// Adds baseline security headers to every response.
///
/// - `X-Frame-Options: DENY` — prevents clickjacking via iframes.
/// - `X-Content-Type-Options: nosniff` — prevents MIME-sniffing attacks.
/// - `Referrer-Policy` — limits referrer leakage to same origin on cross-origin requests.
/// - `Permissions-Policy` — opts out of browser features the app doesn't use.
pub async fn security_headers(req: Request<Body>, next: Next) -> Response {
    let dashboard = is_dashboard_path(req.uri().path());
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert(
        header::HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    headers.insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    headers.insert(
        header::HeaderName::from_static("content-security-policy"),
        if dashboard {
            HeaderValue::from_static(DASHBOARD_CSP)
        } else {
            HeaderValue::from_static(
                "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
                 img-src 'self' data:; connect-src 'self'",
            )
        },
    );
    headers.insert(
        header::HeaderName::from_static("strict-transport-security"),
        HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    );
    response
}

/// The dashboard's policy: the API policy plus the web fonts its
/// `index.html` loads, and no framing or foreign form targets.
const DASHBOARD_CSP: &str = "default-src 'self'; script-src 'self'; \
    style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
    font-src 'self' https://fonts.gstatic.com; img-src 'self' data:; connect-src 'self'; \
    frame-ancestors 'none'; base-uri 'self'; form-action 'self'";

/// Paths served by the dashboard fallback rather than an API route. The
/// dashboard has client-side routes such as `/flags`, so only the exact API
/// paths are excluded.
fn is_dashboard_path(path: &str) -> bool {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    !matches!(
        segments.as_slice(),
        ["api", ..] | ["stream"] | ["flags", "snapshot"] | ["health", ..]
    )
}

/// CSRF defence-in-depth for the dashboard SPA.
///
/// Rejects mutating requests (POST, PUT, PATCH, DELETE) that lack the
/// `X-Checkgate-Request` header, UNLESS the request carries an explicit
/// `Authorization: Bearer` token — Bearer-authenticated calls can never be
/// CSRF'd because the attacker cannot obtain the SDK key from another origin.
pub async fn csrf_protection(req: Request<Body>, next: Next) -> Result<Response, StatusCode> {
    let method = req.method();
    let is_mutating = matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );

    if is_mutating {
        let has_bearer = req
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("Bearer "));

        let has_csrf_header = req
            .headers()
            .get("X-Checkgate-Request")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| !v.is_empty());

        if !has_bearer && !has_csrf_header {
            warn!(
                method = %method,
                path = %req.uri().path(),
                "Rejected mutating request: missing X-Checkgate-Request header"
            );
            return Err(StatusCode::FORBIDDEN);
        }
    }

    Ok(next.run(req).await)
}

/// Read-only API routes — any authenticated user (membership filtered per handler).
pub fn read_router() -> Router<AppState> {
    flags::read_router()
        .merge(impressions::read_router())
        .merge(events::read_router())
        .merge(experiments::read_router())
        .merge(environments::read_router())
        .merge(keys::read_router())
        .merge(users::read_router())
        .merge(projects::read_router())
        .merge(audit::read_router())
        .merge(segments::read_router())
        .merge(webhooks::read_router())
        .merge(integrations::read_router())
        .merge(scheduled::read_router())
        .merge(health::read_router())
        .merge(tokens::read_router())
        .merge(change_requests::read_router())
}

/// Self-service write routes — any authenticated user, no elevated role
/// required. Handlers enforce their own "self only" scoping (e.g. a user
/// managing their own personal access tokens).
pub fn self_service_router() -> Router<AppState> {
    tokens::self_service_router()
}

/// SDK ingest routes — authorized per environment inside the handlers.
/// Placed separately so they can carry a higher body-size limit.
pub fn ingest_router() -> Router<AppState> {
    impressions::ingest_router().merge(events::ingest_router())
}

/// Flag write routes — require editor or admin role (layer added in main.rs).
pub fn editor_write_router() -> Router<AppState> {
    flags::write_router().merge(change_requests::write_router())
}

/// Experiment write routes — require editor or admin role.
pub fn experiment_write_router() -> Router<AppState> {
    experiments::write_router()
}

/// Admin-only write routes: environments, SDK keys, users, projects (layer added in main.rs).
pub fn admin_write_router() -> Router<AppState> {
    environments::write_router()
        .merge(keys::write_router())
        .merge(users::write_router())
        .merge(projects::write_router())
}

/// Segment write routes — require editor or admin role.
pub fn segment_write_router() -> Router<AppState> {
    segments::write_router()
}

/// Webhook write routes — admin only. Chat integrations ride along: they carry
/// a channel-posting credential, so they belong at the same tier.
pub fn webhook_write_router() -> Router<AppState> {
    webhooks::write_router().merge(integrations::write_router())
}

/// Scheduled change write routes — require editor or admin role.
pub fn scheduled_write_router() -> Router<AppState> {
    scheduled::write_router()
}

/// Public auth routes — mounted under `/api/auth` WITHOUT auth middleware.
pub fn auth_router() -> Router<AppState> {
    Router::new()
        .route("/login", post(session::login))
        .route("/logout", post(session::logout))
        .route("/me", get(session::me))
        .route("/workspace", get(session::workspace_info))
}

/// Public setup routes — mounted under `/api` WITHOUT auth middleware.
pub fn setup_router() -> Router<AppState> {
    keys::setup_router().merge(session::setup_router())
}

#[cfg(test)]
mod dashboard_path_tests {
    #[test]
    fn api_paths_are_not_dashboard() {
        for p in [
            "/api/flags",
            "/stream",
            "/flags/snapshot",
            "/health",
            "/health/ready",
        ] {
            assert!(!super::is_dashboard_path(p), "{p}");
        }
        for p in [
            "/",
            "/flags",
            "/flags/new",
            "/assets/index.js",
            "/login",
            "/streams",
        ] {
            assert!(super::is_dashboard_path(p), "{p}");
        }
    }
}
