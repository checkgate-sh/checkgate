use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use axum::{
    Json, Router,
    extract::{ConnectInfo, State},
    http::StatusCode,
    routing::{get, post},
};
use axum_extra::extract::cookie::{Cookie, PrivateCookieJar, SameSite};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::net::SocketAddr;
use time::Duration;
use tracing::{error, info, warn};

use crate::state::AppState;

// ---------------------------------------------------------------------------
// Lockout policy
// ---------------------------------------------------------------------------

/// Maximum failed attempts within the window before the account is locked.
const MAX_ATTEMPTS: i64 = 5;
/// Lockout window in minutes. After this window attempts are forgotten.
const WINDOW_MINUTES: i64 = 10;
/// How long (minutes) the account stays locked after hitting MAX_ATTEMPTS.
const LOCKOUT_MINUTES: i64 = 15;

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

/// Stored encrypted inside the `lg_session` cookie.
#[derive(Serialize, Deserialize)]
struct SessionData {
    email: String,
    name: String,
    role: String,
}

/// Shape returned by `/api/auth/login`, `/api/auth/me`, and `/api/setup/complete`.
#[derive(Serialize)]
pub struct UserInfo {
    pub email: String,
    pub name: String,
    pub role: String,
    pub workspace_name: String,
    pub is_setup_complete: bool,
}

/// Body expected by `POST /api/auth/login`.
#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// Body for the first-run `POST /api/setup/complete`.
#[derive(Deserialize)]
pub struct SetupRequest {
    pub workspace_name: String,
    /// Name for the first project (e.g. "My App"). The migration seeds a
    /// "Default Project" placeholder; setup renames it to this value.
    #[serde(default = "default_project_name")]
    pub project_name: String,
    pub name: String,
    pub email: String,
    pub password: String,
}

fn default_project_name() -> String {
    "Default Project".into()
}

/// Error body returned on lockout so the UI can show a meaningful message.
#[derive(Serialize)]
#[allow(dead_code)]
struct LockoutError {
    error: &'static str,
    retry_after_seconds: i64,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_session_cookie(value: String, secure: bool) -> Cookie<'static> {
    Cookie::build(("lg_session", value))
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(secure)
        .path("/")
        .max_age(Duration::days(7))
        .build()
}

fn cookie_secure() -> bool {
    std::env::var("COOKIE_SECURE")
        .map(|v| !v.eq_ignore_ascii_case("false") && v != "0")
        .unwrap_or(true)
}

pub(crate) fn hash_password(password: &str) -> Result<String, StatusCode> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| {
            error!(error = %e, "Failed to hash password");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// Dummy Argon2id hash used when no real hash is available, so that the "user
/// not found" and "wrong password" paths take the same time. Generated with
/// default Argon2id parameters over an empty input; always fails verification.
const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

async fn get_workspace_name(state: &AppState) -> String {
    sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = 'workspace_name'")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

async fn get_is_setup_complete(state: &AppState) -> bool {
    sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = 'setup_complete'")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Lockout helpers
// ---------------------------------------------------------------------------

/// Returns `Some(retry_after_seconds)` if the account is currently locked,
/// `None` if the login attempt is allowed to proceed.
#[allow(
    clippy::arithmetic_side_effects,
    reason = "`oldest` is a timestamp read back from a NOW()-bounded window, so adding 15 minutes cannot approach the year-9999 overflow point"
)]
async fn check_lockout(state: &AppState, email: &str) -> Result<Option<i64>, StatusCode> {
    // Count failures in the last WINDOW_MINUTES for this email.
    let recent_failures: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM login_attempts \
         WHERE email = $1 AND attempted_at > NOW() - ($2 || ' minutes')::interval",
    )
    .bind(email)
    .bind(WINDOW_MINUTES)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error checking login attempts");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if recent_failures < MAX_ATTEMPTS {
        return Ok(None);
    }

    // Account is locked — calculate how many seconds remain.
    let oldest_in_window: Option<time::OffsetDateTime> = sqlx::query_scalar(
        "SELECT attempted_at FROM login_attempts \
         WHERE email = $1 AND attempted_at > NOW() - ($2 || ' minutes')::interval \
         ORDER BY attempted_at ASC LIMIT 1",
    )
    .bind(email)
    .bind(WINDOW_MINUTES)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error fetching oldest attempt");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let retry_after = if let Some(oldest) = oldest_in_window {
        let unlock_at = oldest + time::Duration::minutes(LOCKOUT_MINUTES);
        let now = time::OffsetDateTime::now_utc();
        (unlock_at - now).whole_seconds().max(1)
    } else {
        LOCKOUT_MINUTES * 60
    };

    Ok(Some(retry_after))
}

/// Records a failed login attempt and purges stale rows older than the window.
async fn record_failure(state: &AppState, email: &str, ip: &str) {
    if let Err(e) = sqlx::query("INSERT INTO login_attempts (email, ip) VALUES ($1, $2)")
        .bind(email)
        .bind(ip)
        .execute(&state.db)
        .await
    {
        error!(error = %e, "Failed to record login attempt");
    }

    // Best-effort cleanup: delete rows older than the lockout window to keep
    // the table from growing unbounded.
    let _ = sqlx::query(
        "DELETE FROM login_attempts \
         WHERE attempted_at < NOW() - ($1 || ' minutes')::interval",
    )
    .bind(WINDOW_MINUTES * 2)
    .execute(&state.db)
    .await;
}

/// Clears all failed login attempts for the email on successful login.
async fn clear_attempts(state: &AppState, email: &str) {
    let _ = sqlx::query("DELETE FROM login_attempts WHERE email = $1")
        .bind(email)
        .execute(&state.db)
        .await;
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Validate email + password, enforce lockout, issue session cookie.
#[allow(
    clippy::arithmetic_side_effects,
    reason = "MAX_ATTEMPTS and `failures` are both i64; a negative difference is clamped by .max(0) rather than underflowing"
)]
pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    jar: PrivateCookieJar,
    Json(req): Json<LoginRequest>,
) -> Result<(PrivateCookieJar, Json<UserInfo>), (StatusCode, Json<serde_json::Value>)> {
    let email = req.email.trim().to_lowercase();
    let ip = addr.ip().to_string();

    if email.is_empty() || req.password.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": "Email and password are required." })),
        ));
    }

    // ── Lockout check (before DB user lookup to avoid user enumeration) ───────
    let lockout = check_lockout(&state, &email)
        .await
        .map_err(|s| (s, Json(serde_json::json!({ "error": "Internal error." }))))?;

    if let Some(retry_after) = lockout {
        warn!(
            email = %email,
            ip = %ip,
            retry_after_seconds = retry_after,
            "Login blocked: account locked out"
        );
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": "Too many failed attempts. Account temporarily locked.",
                "retry_after_seconds": retry_after
            })),
        ));
    }

    // ── User lookup ───────────────────────────────────────────────────────────
    let row = sqlx::query("SELECT name, role, password_hash FROM users WHERE email = $1")
        .bind(&email)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| {
            error!(error = %e, "DB error during login user lookup");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Internal error." })),
            )
        })?;

    // ── Credential validation ─────────────────────────────────────────────────
    // Always call verify_password — even when the user doesn't exist or has no
    // password hash — so that "user not found" and "wrong password" take the same
    // time and cannot be distinguished by a timing side-channel.
    let auth_ok = match &row {
        None => {
            verify_password(&req.password, DUMMY_HASH);
            false
        }
        Some(r) => {
            let hash: Option<String> = r.get("password_hash");
            match hash {
                None => {
                    verify_password(&req.password, DUMMY_HASH);
                    false
                }
                Some(h) => verify_password(&req.password, &h),
            }
        }
    };

    if !auth_ok {
        // Record failure regardless of whether the user exists.
        record_failure(&state, &email, &ip).await;

        // Re-check lockout count to tell the user how many attempts remain.
        let failures: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM login_attempts \
             WHERE email = $1 AND attempted_at > NOW() - ($2 || ' minutes')::interval",
        )
        .bind(&email)
        .bind(WINDOW_MINUTES)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

        let remaining = (MAX_ATTEMPTS - failures).max(0);

        warn!(
            email = %email,
            ip = %ip,
            failures = failures,
            remaining_attempts = remaining,
            "Login failed: invalid credentials"
        );

        if remaining == 0 {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "error": "Too many failed attempts. Account locked for 15 minutes.",
                    "retry_after_seconds": LOCKOUT_MINUTES * 60
                })),
            ));
        }

        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "Incorrect email or password.",
                "attempts_remaining": remaining
            })),
        ));
    }

    // ── Success ───────────────────────────────────────────────────────────────
    // `auth_ok` can only be true in the `Some(r)` arm above, so this always
    // binds. Written as a `let ... else` rather than an `unwrap` so that a future
    // change to the matching above degrades to a 500 instead of panicking the
    // request handler.
    let Some(row) = row else {
        error!(email = %email, "auth_ok with no user row — unreachable");
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Internal error." })),
        ));
    };
    let name: String = row.get("name");
    let role: String = row.get("role");

    // Clear failure history on successful login.
    clear_attempts(&state, &email).await;

    let workspace_name = get_workspace_name(&state).await;
    let is_setup_complete = get_is_setup_complete(&state).await;

    let session = SessionData {
        email: email.clone(),
        name: name.clone(),
        role: role.clone(),
    };
    let cookie_value = serde_json::to_string(&session).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Internal error." })),
        )
    })?;

    info!(email = %email, role = %role, ip = %ip, "Session created");

    Ok((
        jar.add(build_session_cookie(cookie_value, cookie_secure())),
        Json(UserInfo {
            email,
            name,
            role,
            workspace_name,
            is_setup_complete,
        }),
    ))
}

/// Clear the session cookie.
pub async fn logout(jar: PrivateCookieJar) -> (PrivateCookieJar, StatusCode) {
    let jar = jar.remove(Cookie::build(("lg_session", "")).path("/"));
    (jar, StatusCode::NO_CONTENT)
}

/// Return the current user from the session cookie.
pub async fn me(
    State(state): State<AppState>,
    jar: PrivateCookieJar,
) -> Result<Json<UserInfo>, StatusCode> {
    let cookie = jar.get("lg_session").ok_or(StatusCode::UNAUTHORIZED)?;
    let session: SessionData =
        serde_json::from_str(cookie.value()).map_err(|_| StatusCode::UNAUTHORIZED)?;

    let workspace_name = get_workspace_name(&state).await;
    let is_setup_complete = get_is_setup_complete(&state).await;

    Ok(Json(UserInfo {
        email: session.email,
        name: session.name,
        role: session.role,
        workspace_name,
        is_setup_complete,
    }))
}

/// First-run setup: creates the admin user and workspace, issues a session cookie.
pub async fn setup_complete(
    State(state): State<AppState>,
    jar: PrivateCookieJar,
    Json(req): Json<SetupRequest>,
) -> Result<(PrivateCookieJar, Json<UserInfo>), StatusCode> {
    let complete =
        sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = 'setup_complete'")
            .fetch_optional(&state.db)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .map(|v| v == "true")
            .unwrap_or(false);

    if complete {
        return Err(StatusCode::NOT_FOUND);
    }

    let workspace_name = req.workspace_name.trim().to_string();
    let name = req.name.trim().to_string();
    let email = req.email.trim().to_lowercase();

    if workspace_name.is_empty() || name.is_empty() || email.is_empty() || req.password.is_empty() {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    if req.password.len() < 8 {
        warn!("Setup rejected: password too short");
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let password_hash = hash_password(&req.password)?;

    let mut db_tx = state.db.begin().await.map_err(|e| {
        error!(error = %e, "Failed to begin setup transaction");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    sqlx::query(
        "INSERT INTO users (name, email, role, password_hash) VALUES ($1, $2, 'admin', $3) \
         ON CONFLICT (email) DO UPDATE SET name = EXCLUDED.name, role = 'admin', \
         password_hash = EXCLUDED.password_hash",
    )
    .bind(&name)
    .bind(&email)
    .bind(&password_hash)
    .execute(&mut *db_tx)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to create admin user during setup");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('workspace_name', $1) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(&workspace_name)
    .execute(&mut *db_tx)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to store workspace name");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Rename the seeded Default Project to the user's chosen name.
    let project_name = req.project_name.trim().to_string();
    if !project_name.is_empty() {
        sqlx::query("UPDATE projects SET name = $1 WHERE slug = 'default'")
            .bind(&project_name)
            .execute(&mut *db_tx)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to rename default project during setup");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }

    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('setup_complete', 'true') \
         ON CONFLICT (key) DO UPDATE SET value = 'true'",
    )
    .execute(&mut *db_tx)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to mark setup complete");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    db_tx.commit().await.map_err(|e| {
        error!(error = %e, "Failed to commit setup transaction");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    info!(email = %email, workspace = %workspace_name, project = %project_name, "Setup complete — admin user created");

    let session = SessionData {
        email: email.clone(),
        name: name.clone(),
        role: "admin".into(),
    };
    let cookie_value =
        serde_json::to_string(&session).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((
        jar.add(build_session_cookie(cookie_value, cookie_secure())),
        Json(UserInfo {
            email,
            name,
            role: "admin".into(),
            workspace_name,
            is_setup_complete: true,
        }),
    ))
}

/// Returns workspace name and setup status — public (no session required),
/// so the frontend can tell "setup already done, go to /login" apart from
/// "setup not done yet" *before* the user has any session cookie. `/api/auth/me`
/// can't be used for this on its own: it 401s for anyone without a valid
/// session, which is exactly the case of someone who needs to log in.
pub async fn workspace_info(State(state): State<AppState>) -> Json<serde_json::Value> {
    let workspace_name = get_workspace_name(&state).await;
    let is_setup_complete = get_is_setup_complete(&state).await;
    Json(
        serde_json::json!({ "workspace_name": workspace_name, "is_setup_complete": is_setup_complete }),
    )
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn setup_router() -> Router<AppState> {
    Router::new().route("/setup/complete", post(setup_complete))
}

#[allow(dead_code)]
pub fn auth_router() -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/me", get(me))
        .route("/workspace", get(workspace_info))
}
