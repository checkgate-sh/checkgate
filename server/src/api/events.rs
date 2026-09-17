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
use tracing::{error, info, warn};

const MAX_BATCH: usize = 500;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

// See impressions.rs for why `time::serde::rfc3339` is used on every
// OffsetDateTime field: JavaScript's `Date` cannot parse `time`'s default
// serialization, so RFC 3339 is applied explicitly.

/// One conversion/goal event reported by an SDK client via `track()`.
#[derive(Debug, Deserialize)]
pub struct EventPayload {
    pub event_key: String,
    pub user_id: Option<String>,
    /// Optional numeric payload (revenue, count, duration…).
    pub value: Option<f64>,
    pub context: Option<serde_json::Value>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub occurred_at: Option<time::OffsetDateTime>,
}

/// A distinct goal-event key with a usage count — powers the experiment
/// goal-event dropdown in the dashboard.
#[derive(Debug, Serialize)]
pub struct EventKeyInfo {
    pub event_key: String,
    pub total: i64,
    pub unique_users: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_seen: Option<time::OffsetDateTime>,
}

// ---------------------------------------------------------------------------
// Routers
// ---------------------------------------------------------------------------

/// Event read routes — any authenticated user.
pub fn read_router() -> Router<AppState> {
    Router::new().route("/environments/{env_id}/events/keys", get(list_event_keys))
}

/// Event ingest route — any authenticated client (including SDK Bearer keys).
pub fn ingest_router() -> Router<AppState> {
    Router::new().route("/environments/{env_id}/events", post(ingest_events))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/environments/{env_id}/events
///
/// Accepts a JSON array of goal/conversion events from SDK clients.
/// Called asynchronously after `track()` — fire-and-forget from the SDK side,
/// mirroring impression ingest.
async fn ingest_events(
    State(state): State<AppState>,
    Path(env_id): Path<String>,
    Json(batch): Json<Vec<EventPayload>>,
) -> Result<StatusCode, StatusCode> {
    if batch.is_empty() {
        return Ok(StatusCode::NO_CONTENT);
    }

    if batch.len() > MAX_BATCH {
        warn!(
            count = batch.len(),
            max = MAX_BATCH,
            "Event batch too large — rejected"
        );
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }

    let env_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM environments WHERE id = $1::uuid)")
            .bind(&env_id)
            .fetch_one(&state.db)
            .await
            .map_err(|e| {
                error!(error = %e, "DB error checking environment");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

    if !env_exists {
        return Err(StatusCode::NOT_FOUND);
    }

    let mut tx = state.db.begin().await.map_err(|e| {
        error!(error = %e, "Failed to begin transaction");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut accepted = 0usize;

    for ev in &batch {
        if ev.event_key.is_empty() || ev.event_key.len() > 100 {
            continue;
        }
        let occurred_at = ev.occurred_at.unwrap_or_else(time::OffsetDateTime::now_utc);

        // Clamp the client-supplied time to the server clock (`LEAST(_, NOW())`):
        // a future-fast client clock must never let a conversion sort ahead of
        // the exposure that caused it, which would corrupt experiment attribution.
        // Genuinely-old times — e.g. offline events queued while disconnected —
        // are left untouched.
        sqlx::query(
            "INSERT INTO events \
             (environment_id, event_key, user_id, value, context, occurred_at) \
             VALUES ($1::uuid, $2, $3, $4, $5, LEAST($6, NOW()))",
        )
        .bind(&env_id)
        .bind(&ev.event_key)
        .bind(&ev.user_id)
        .bind(ev.value)
        .bind(&ev.context)
        .bind(occurred_at)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to insert event");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        accepted = accepted.saturating_add(1);
    }

    tx.commit().await.map_err(|e| {
        error!(error = %e, "Transaction commit failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    info!(env_id = %env_id, accepted, total = batch.len(), "Events ingested");
    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/environments/{env_id}/events/keys
///
/// Returns the distinct goal-event keys seen in this environment, each with a
/// usage count. Used to populate the goal-event picker when creating an
/// experiment.
async fn list_event_keys(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(env_id): Path<String>,
) -> Result<Json<Vec<EventKeyInfo>>, StatusCode> {
    super::flags::check_env_access(&state.db, &jar, &env_id).await?;

    let rows = sqlx::query(
        "SELECT event_key, \
                COUNT(*)                AS total, \
                COUNT(DISTINCT user_id) AS unique_users, \
                MAX(occurred_at)        AS last_seen \
         FROM events \
         WHERE environment_id = $1::uuid \
         GROUP BY event_key \
         ORDER BY total DESC",
    )
    .bind(&env_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error listing event keys");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let keys: Vec<EventKeyInfo> = rows
        .iter()
        .map(|r| EventKeyInfo {
            event_key: r.get("event_key"),
            total: r.get("total"),
            unique_users: r.get("unique_users"),
            last_seen: r.get("last_seen"),
        })
        .collect();

    Ok(Json(keys))
}
