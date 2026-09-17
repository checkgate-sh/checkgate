use crate::auth::AuthContext;
use crate::state::AppState;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
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

// `time`'s default (de)serialization is a proprietary space-separated,
// triple-colon-offset format that JavaScript's `Date` cannot parse —
// `time::serde::rfc3339` gives proper RFC 3339 (`"2026-07-04T03:19:17Z"`)
// on every OffsetDateTime field in this module.

#[derive(Debug, Deserialize)]
pub struct ImpressionPayload {
    pub flag_key: String,
    pub user_id: Option<String>,
    pub value: String,
    pub context: Option<serde_json::Value>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub evaluated_at: Option<time::OffsetDateTime>,
}

#[derive(Debug, Serialize)]
pub struct Impression {
    pub id: i64,
    pub flag_key: String,
    pub user_id: Option<String>,
    pub value: String,
    pub context: Option<serde_json::Value>,
    #[serde(with = "time::serde::rfc3339")]
    pub evaluated_at: time::OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct ImpressionStats {
    pub flag_key: String,
    pub total: i64,
    pub true_count: i64,
    pub false_count: i64,
    pub unique_users: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_seen: Option<time::OffsetDateTime>,
}

#[derive(Debug, Serialize)]
pub struct ImpressionListResponse {
    pub items: Vec<Impression>,
    pub total: i64,
}

// --- Exposure dashboard types ---------------------------------------------

/// Per-variant exposure for a single flag: how many evaluations resolved to
/// this value and how many distinct users saw it.
#[derive(Debug, Serialize)]
pub struct ExposureVariant {
    pub value: String,
    pub impressions: i64,
    pub unique_users: i64,
}

/// One point on the daily exposure timeline: evaluations of `value` on `day`.
#[derive(Debug, Serialize)]
pub struct ExposurePoint {
    pub day: String,
    pub value: String,
    pub count: i64,
}

/// Exposure breakdown for one flag — which users are being exposed to which
/// variant, over the whole retained window plus a recent daily timeline.
#[derive(Debug, Serialize)]
pub struct ExposureResponse {
    pub flag_key: String,
    pub total_impressions: i64,
    pub total_users: i64,
    pub variants: Vec<ExposureVariant>,
    pub timeline: Vec<ExposurePoint>,
}

#[derive(Debug, Deserialize)]
pub struct ExposureQuery {
    pub flag_key: String,
    /// Number of trailing days to include in the daily timeline (1–90).
    #[serde(default = "default_exposure_days")]
    pub days: i64,
}

fn default_exposure_days() -> i64 {
    14
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub flag_key: Option<String>,
    /// Filter by exact user_id.
    pub user_id: Option<String>,
    /// Filter by exact evaluated value (e.g. "true", "false", or a variant string).
    pub value: Option<String>,
    /// Return only rows with id > since_id — used by live stream polling to fetch
    /// only new evaluations since the last poll.
    pub since_id: Option<i64>,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

// ---------------------------------------------------------------------------
// Routers
// ---------------------------------------------------------------------------

/// Impression read routes — any authenticated user.
pub fn read_router() -> Router<AppState> {
    Router::new()
        .route("/environments/{env_id}/impressions", get(list_impressions))
        .route(
            "/environments/{env_id}/impressions/stats",
            get(impression_stats),
        )
        .route("/environments/{env_id}/impressions/exposure", get(exposure))
}

/// Impression ingest route — any authenticated client (including SDK Bearer keys).
pub fn ingest_router() -> Router<AppState> {
    Router::new().route(
        "/environments/{env_id}/impressions",
        post(ingest_impressions),
    )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/environments/{env_id}/impressions
///
/// Accepts a JSON array of evaluation events from SDK clients.
/// Called asynchronously after flag evaluations — fire-and-forget from SDK side.
async fn ingest_impressions(
    State(state): State<AppState>,
    Path(env_id): Path<String>,
    Json(batch): Json<Vec<ImpressionPayload>>,
) -> Result<StatusCode, StatusCode> {
    if batch.is_empty() {
        return Ok(StatusCode::NO_CONTENT);
    }

    if batch.len() > MAX_BATCH {
        warn!(
            count = batch.len(),
            max = MAX_BATCH,
            "Impression batch too large — rejected"
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

    for imp in &batch {
        if imp.flag_key.is_empty() || imp.flag_key.len() > 100 {
            continue;
        }
        let evaluated_at = imp
            .evaluated_at
            .unwrap_or_else(time::OffsetDateTime::now_utc);

        // Clamp the client-supplied time to the server clock (`LEAST(_, NOW())`):
        // a future-fast client clock must never let an evaluation sort ahead of
        // reality, which would corrupt experiment attribution (a conversion could
        // appear to precede its exposure). Genuinely-old times — e.g. offline
        // events queued while disconnected — are left untouched.
        sqlx::query(
            "INSERT INTO impressions \
             (environment_id, flag_key, user_id, value, context, evaluated_at) \
             VALUES ($1::uuid, $2, $3, $4, $5, LEAST($6, NOW()))",
        )
        .bind(&env_id)
        .bind(&imp.flag_key)
        .bind(&imp.user_id)
        .bind(&imp.value)
        .bind(&imp.context)
        .bind(evaluated_at)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to insert impression");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        accepted = accepted.saturating_add(1);
    }

    tx.commit().await.map_err(|e| {
        error!(error = %e, "Transaction commit failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    info!(env_id = %env_id, accepted, total = batch.len(), "Impressions ingested");
    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/environments/{env_id}/impressions
///
/// Returns a paginated list of recent evaluations.
///
/// Optional filters: `flag_key`, `user_id`, `value`, `since_id`.
/// `since_id` is for live polling — returns only rows with `id > since_id`.
/// `total` always reflects the count matching the base filters (ignoring `since_id`).
async fn list_impressions(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(env_id): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<Json<ImpressionListResponse>, StatusCode> {
    super::flags::check_env_access(&state.db, &jar, &env_id).await?;
    let limit = q.limit.clamp(1, 200);
    let offset = q.offset.max(0);

    // Unified parameterized query — NULL params act as "no filter".
    // $2=flag_key, $3=user_id, $4=value, $5=since_id
    let rows = sqlx::query(
        "SELECT id, flag_key, user_id, value, context, evaluated_at \
         FROM impressions \
         WHERE environment_id = $1::uuid \
           AND ($2::text   IS NULL OR flag_key = $2) \
           AND ($3::text   IS NULL OR user_id  = $3) \
           AND ($4::text   IS NULL OR value    = $4) \
           AND ($5::bigint IS NULL OR id       > $5) \
         ORDER BY evaluated_at DESC, id DESC \
         LIMIT $6 OFFSET $7",
    )
    .bind(&env_id)
    .bind(q.flag_key.as_deref())
    .bind(q.user_id.as_deref())
    .bind(q.value.as_deref())
    .bind(q.since_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error listing impressions");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Total ignores since_id so the client can display "X total" independent of polling state.
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM impressions \
         WHERE environment_id = $1::uuid \
           AND ($2::text IS NULL OR flag_key = $2) \
           AND ($3::text IS NULL OR user_id  = $3) \
           AND ($4::text IS NULL OR value    = $4)",
    )
    .bind(&env_id)
    .bind(q.flag_key.as_deref())
    .bind(q.user_id.as_deref())
    .bind(q.value.as_deref())
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error counting impressions");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let items: Vec<Impression> = rows
        .iter()
        .map(|r| Impression {
            id: r.get("id"),
            flag_key: r.get("flag_key"),
            user_id: r.get("user_id"),
            value: r.get("value"),
            context: r.get("context"),
            evaluated_at: r.get("evaluated_at"),
        })
        .collect();

    Ok(Json(ImpressionListResponse { items, total }))
}

/// GET /api/environments/{env_id}/impressions/stats
///
/// Returns per-flag aggregate counts: total evals, true/false split, unique users.
async fn impression_stats(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(env_id): Path<String>,
) -> Result<Json<Vec<ImpressionStats>>, StatusCode> {
    super::flags::check_env_access(&state.db, &jar, &env_id).await?;
    let rows = sqlx::query(
        "SELECT flag_key, \
                COUNT(*) AS total, \
                COUNT(*) FILTER (WHERE value = 'true')  AS true_count, \
                COUNT(*) FILTER (WHERE value = 'false') AS false_count, \
                COUNT(DISTINCT user_id)                 AS unique_users, \
                MAX(evaluated_at)                       AS last_seen \
         FROM impressions \
         WHERE environment_id = $1::uuid \
         GROUP BY flag_key \
         ORDER BY total DESC",
    )
    .bind(&env_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error fetching impression stats");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let stats: Vec<ImpressionStats> = rows
        .iter()
        .map(|r| ImpressionStats {
            flag_key: r.get("flag_key"),
            total: r.get("total"),
            true_count: r.get("true_count"),
            false_count: r.get("false_count"),
            unique_users: r.get("unique_users"),
            last_seen: r.get("last_seen"),
        })
        .collect();

    Ok(Json(stats))
}

/// GET /api/environments/{env_id}/impressions/exposure?flag_key=X&days=14
///
/// Exposure breakdown for a single flag: per-variant impression and unique-user
/// counts, plus a daily timeline of evaluations per variant. Powers the
/// Exposure dashboard ("which users are being exposed to which variant?").
async fn exposure(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(env_id): Path<String>,
    Query(q): Query<ExposureQuery>,
) -> Result<Json<ExposureResponse>, StatusCode> {
    super::flags::check_env_access(&state.db, &jar, &env_id).await?;

    if q.flag_key.is_empty() || q.flag_key.len() > 100 {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let days = q.days.clamp(1, 90);

    // Per-variant totals across the whole retained window.
    let variant_rows = sqlx::query(
        "SELECT value, \
                COUNT(*)                AS impressions, \
                COUNT(DISTINCT user_id) AS unique_users \
         FROM impressions \
         WHERE environment_id = $1::uuid AND flag_key = $2 \
         GROUP BY value \
         ORDER BY impressions DESC",
    )
    .bind(&env_id)
    .bind(&q.flag_key)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error computing exposure variants");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let variants: Vec<ExposureVariant> = variant_rows
        .iter()
        .map(|r| ExposureVariant {
            value: r.get("value"),
            impressions: r.get("impressions"),
            unique_users: r.get("unique_users"),
        })
        .collect();

    let total_impressions: i64 = variants.iter().map(|v| v.impressions).sum();

    let total_users: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT user_id) FROM impressions \
         WHERE environment_id = $1::uuid AND flag_key = $2",
    )
    .bind(&env_id)
    .bind(&q.flag_key)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error computing exposure user total");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Daily timeline for the trailing `days` window.
    let timeline_rows = sqlx::query(
        "SELECT to_char(date_trunc('day', evaluated_at), 'YYYY-MM-DD') AS day, \
                value, COUNT(*) AS count \
         FROM impressions \
         WHERE environment_id = $1::uuid AND flag_key = $2 \
           AND evaluated_at >= NOW() - ($3 || ' days')::interval \
         GROUP BY day, value \
         ORDER BY day ASC",
    )
    .bind(&env_id)
    .bind(&q.flag_key)
    .bind(days.to_string())
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error computing exposure timeline");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let timeline: Vec<ExposurePoint> = timeline_rows
        .iter()
        .map(|r| ExposurePoint {
            day: r.get("day"),
            value: r.get("value"),
            count: r.get("count"),
        })
        .collect();

    Ok(Json(ExposureResponse {
        flag_key: q.flag_key,
        total_impressions,
        total_users,
        variants,
        timeline,
    }))
}
