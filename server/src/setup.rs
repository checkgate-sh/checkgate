//! First-run ownership: the one-time setup token.
//!
//! Until setup completes, `/api/setup/*` creates the first workspace admin, so
//! whoever reaches a fresh installation first would otherwise own it. Those
//! routes therefore require a setup token in the `X-Checkgate-Setup-Token`
//! header — never a query parameter, so it cannot leak into access logs.
//!
//! The token comes from one of two places:
//!
//! - `SETUP_TOKEN` — chosen by the operator. Every instance must share it.
//! - Otherwise, each instance generates a random token on boot while setup is
//!   incomplete, writes it to `SETUP_TOKEN_FILE` (mode 0600) and stores only
//!   its SHA-256 in `settings`, so any instance accepts any generated token.
//!   The token itself is never logged.
//!
//! Completing setup deletes every stored token hash in the same transaction
//! that marks setup complete.

use crate::state::AppState;
use axum::http::{HeaderMap, StatusCode};
use constant_time_eq::constant_time_eq;
use rand::RngExt as _;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};

pub const SETUP_TOKEN_HEADER: &str = "x-checkgate-setup-token";

/// Shortest operator-chosen token accepted. Setup grants full admin rights.
const MIN_TOKEN_LEN: usize = 16;

/// `settings` key prefix for generated token hashes.
const TOKEN_KEY_PREFIX: &str = "setup_token:";

/// Key for the transaction-scoped advisory lock that serializes setup across
/// every server instance sharing the database.
pub const SETUP_ADVISORY_LOCK: i64 = 0x636b_6774_5f73_6574; // "ckgt_set"

/// Reads `SETUP_TOKEN`. Unset or empty means "generate one per instance".
pub fn configured_token() -> Result<Option<Arc<str>>, String> {
    match std::env::var("SETUP_TOKEN") {
        Ok(token) if !token.trim().is_empty() => {
            let token = token.trim();
            if token.len() < MIN_TOKEN_LEN {
                return Err(format!(
                    "SETUP_TOKEN must be at least {MIN_TOKEN_LEN} characters"
                ));
            }
            Ok(Some(Arc::from(token)))
        }
        _ => Ok(None),
    }
}

fn token_file() -> PathBuf {
    std::env::var_os("SETUP_TOKEN_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("checkgate-setup-token"))
}

pub(crate) async fn is_setup_complete<'e>(
    db: impl sqlx::PgExecutor<'e>,
) -> Result<bool, sqlx::Error> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = 'setup_complete'")
            .fetch_optional(db)
            .await?
            .is_some_and(|v| v == "true"),
    )
}

/// Startup step: make sure a fresh installation can be claimed by its
/// operator, and only by its operator.
pub async fn prepare(db: &sqlx::PgPool, configured: Option<&str>) -> Result<(), String> {
    let complete = is_setup_complete(db)
        .await
        .map_err(|e| format!("failed to read setup state: {e}"))?;
    let path = token_file();

    if complete {
        // Nothing to claim: a token file left from before setup is just a
        // stale secret on disk.
        if path.exists() {
            match std::fs::remove_file(&path) {
                Ok(()) => info!(path = %path.display(), "Removed stale setup token file"),
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "Could not remove stale setup token file")
                }
            }
        }
        return Ok(());
    }

    if configured.is_some() {
        info!("Setup is not complete — the setup wizard requires the SETUP_TOKEN value");
        return Ok(());
    }

    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

    write_token_file(&path, &token).map_err(|e| {
        format!(
            "setup is not complete and the setup token could not be written to {} ({e}); \
             set SETUP_TOKEN, or SETUP_TOKEN_FILE to a writable path",
            path.display()
        )
    })?;

    sqlx::query("INSERT INTO settings (key, value) VALUES ($1, NOW()::text)")
        .bind(format!(
            "{TOKEN_KEY_PREFIX}{}",
            crate::auth::hash_token(&token)
        ))
        .execute(db)
        .await
        .map_err(|e| format!("failed to store setup token: {e}"))?;

    info!(
        path = %path.display(),
        "Setup is not complete — a one-time setup token was written to this file; \
         enter it in the setup wizard"
    );
    Ok(())
}

fn write_token_file(path: &std::path::Path, token: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    // `mode` only applies on creation; tighten a file that already existed.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(token.as_bytes())?;
    file.write_all(b"\n")
}

/// Accepts the request only if it carries a valid setup token.
pub(crate) async fn verify(state: &AppState, headers: &HeaderMap) -> Result<(), StatusCode> {
    let presented = headers
        .get(SETUP_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let valid = match &state.setup_token {
        Some(expected) => constant_time_eq(presented.as_bytes(), expected.as_bytes()),
        // Looked up by hash, so the comparison leaks nothing about the token.
        None => {
            sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM settings WHERE key = $1)")
                .bind(format!(
                    "{TOKEN_KEY_PREFIX}{}",
                    crate::auth::hash_token(presented)
                ))
                .fetch_one(&state.db)
                .await
                .map_err(|e| {
                    error!(error = %e, "DB error verifying setup token");
                    StatusCode::INTERNAL_SERVER_ERROR
                })?
        }
    };
    if valid {
        Ok(())
    } else {
        warn!("Rejected setup request: invalid setup token");
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// Deletes every generated token hash. Runs inside the setup transaction.
pub(crate) async fn revoke_all<'e>(db: impl sqlx::PgExecutor<'e>) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM settings WHERE starts_with(key, 'setup_token:')")
        .execute(db)
        .await
        .map(|_| ())
}

/// Best-effort removal of this instance's token file once setup is done.
pub(crate) fn remove_token_file() {
    let path = token_file();
    if path.exists()
        && let Err(e) = std::fs::remove_file(&path)
    {
        warn!(path = %path.display(), error = %e, "Could not remove setup token file");
    }
}
