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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experiment {
    pub id: String,
    pub environment_id: String,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub flag_key: String,
    pub goal_event_key: String,
    pub control_variant: Option<String>,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
struct CreateExperimentBody {
    name: String,
    key: String,
    #[serde(default)]
    description: Option<String>,
    flag_key: String,
    goal_event_key: String,
    #[serde(default)]
    control_variant: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PatchExperimentBody {
    name: Option<String>,
    description: Option<String>,
    goal_event_key: Option<String>,
    control_variant: Option<String>,
    status: Option<String>,
}

// --- Results ---------------------------------------------------------------

/// Per-variant experiment results with a significance comparison against the
/// control variant.
#[derive(Debug, Serialize)]
pub struct VariantResult {
    pub variant: String,
    /// Distinct users assigned to this variant (by their latest impression).
    pub exposed: i64,
    /// Of those, how many fired the goal event.
    pub converted: i64,
    /// converted / exposed, in `[0, 1]`.
    pub conversion_rate: f64,
    /// True for the baseline variant the others are compared against.
    pub is_control: bool,
    /// Relative uplift vs. control, e.g. `0.12` = +12%. `None` for the control
    /// itself or when the control has a zero conversion rate.
    pub uplift: Option<f64>,
    /// Two-proportion z-score vs. control. `None` for the control or when a
    /// variance of zero makes the test undefined.
    pub z_score: Option<f64>,
    /// Two-sided p-value for the z-score. `None` when `z_score` is `None`.
    pub p_value: Option<f64>,
    /// `true` when `p_value < 0.05` — a conventional 95% significance gate.
    pub significant: bool,
}

#[derive(Debug, Serialize)]
pub struct ExperimentResults {
    pub experiment: Experiment,
    pub control_variant: Option<String>,
    pub total_exposed: i64,
    pub total_converted: i64,
    pub variants: Vec<VariantResult>,
}

// ---------------------------------------------------------------------------
// Key validation — same constraints as flag / segment keys
// ---------------------------------------------------------------------------

fn is_valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 100
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn is_valid_status(status: &str) -> bool {
    matches!(status, "running" | "paused" | "completed")
}

// ---------------------------------------------------------------------------
// Routers
// ---------------------------------------------------------------------------

pub fn read_router() -> Router<AppState> {
    Router::new()
        .route("/environments/{env_id}/experiments", get(list_experiments))
        .route(
            "/environments/{env_id}/experiments/{key}",
            get(get_experiment),
        )
        .route(
            "/environments/{env_id}/experiments/{key}/results",
            get(experiment_results),
        )
}

pub fn write_router() -> Router<AppState> {
    Router::new()
        .route(
            "/environments/{env_id}/experiments",
            post(create_experiment),
        )
        .route(
            "/environments/{env_id}/experiments/{key}",
            axum::routing::patch(patch_experiment).delete(delete_experiment),
        )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

const SELECT_COLS: &str = "id::text, environment_id::text, key, name, description, \
     flag_key, goal_event_key, control_variant, status, created_at::text";

fn row_to_experiment(r: &sqlx::postgres::PgRow) -> Experiment {
    Experiment {
        id: r.get("id"),
        environment_id: r.get("environment_id"),
        key: r.get("key"),
        name: r.get("name"),
        description: r.get("description"),
        flag_key: r.get("flag_key"),
        goal_event_key: r.get("goal_event_key"),
        control_variant: r.get("control_variant"),
        status: r.get("status"),
        created_at: r.get("created_at"),
    }
}

async fn list_experiments(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(env_id): Path<String>,
) -> Result<Json<Vec<Experiment>>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    let rows = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM experiments \
         WHERE environment_id = $1::uuid ORDER BY created_at DESC"
    ))
    .bind(&env_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to list experiments");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(rows.iter().map(row_to_experiment).collect()))
}

async fn get_experiment(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, key)): Path<(String, String)>,
) -> Result<Json<Experiment>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    let row = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM experiments \
         WHERE environment_id = $1::uuid AND key = $2"
    ))
    .bind(&env_id)
    .bind(&key)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error fetching experiment");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(row_to_experiment(&row)))
}

async fn create_experiment(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(env_id): Path<String>,
    Json(body): Json<CreateExperimentBody>,
) -> Result<Json<Experiment>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;
    super::flags::check_env_write_access(&state.db, &jar, &env_id).await?;

    if !is_valid_key(&body.key)
        || !is_valid_key(&body.flag_key)
        || !is_valid_key(&body.goal_event_key)
        || body.name.trim().is_empty()
    {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let row = sqlx::query(&format!(
        "INSERT INTO experiments \
         (environment_id, key, name, description, flag_key, goal_event_key, control_variant) \
         VALUES ($1::uuid, $2, $3, $4, $5, $6, $7) \
         RETURNING {SELECT_COLS}"
    ))
    .bind(&env_id)
    .bind(&body.key)
    .bind(&body.name)
    .bind(&body.description)
    .bind(&body.flag_key)
    .bind(&body.goal_event_key)
    .bind(&body.control_variant)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        // Unique violation → duplicate key in this environment.
        if let sqlx::Error::Database(db_err) = &e
            && db_err.is_unique_violation()
        {
            return StatusCode::CONFLICT;
        }
        error!(error = %e, "Failed to insert experiment");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let exp = row_to_experiment(&row);
    info!(env_id = %env_id, key = %exp.key, "Experiment created");
    Ok(Json(exp))
}

async fn patch_experiment(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, key)): Path<(String, String)>,
    Json(body): Json<PatchExperimentBody>,
) -> Result<Json<Experiment>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;
    super::flags::check_env_write_access(&state.db, &jar, &env_id).await?;

    if let Some(ref g) = body.goal_event_key
        && !is_valid_key(g)
    {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    if let Some(ref s) = body.status
        && !is_valid_status(s)
    {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let row = sqlx::query(&format!(
        "UPDATE experiments SET \
         name            = COALESCE($1, name), \
         description     = COALESCE($2, description), \
         goal_event_key  = COALESCE($3, goal_event_key), \
         control_variant = COALESCE($4, control_variant), \
         status          = COALESCE($5, status) \
         WHERE environment_id = $6::uuid AND key = $7 \
         RETURNING {SELECT_COLS}"
    ))
    .bind(&body.name)
    .bind(&body.description)
    .bind(&body.goal_event_key)
    .bind(&body.control_variant)
    .bind(&body.status)
    .bind(&env_id)
    .bind(&key)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to update experiment");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

    info!(env_id = %env_id, key = %key, "Experiment updated");
    Ok(Json(row_to_experiment(&row)))
}

async fn delete_experiment(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, key)): Path<(String, String)>,
) -> Result<StatusCode, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;
    super::flags::check_env_write_access(&state.db, &jar, &env_id).await?;

    let result =
        sqlx::query("DELETE FROM experiments WHERE environment_id = $1::uuid AND key = $2")
            .bind(&env_id)
            .bind(&key)
            .execute(&state.db)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to delete experiment");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

    if result.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    info!(env_id = %env_id, key = %key, "Experiment deleted");
    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/environments/{env_id}/experiments/{key}/results
///
/// Computes conversion rates per flag variant and a two-proportion significance
/// test of each variant against the control. Each user enters the experiment at
/// their FIRST exposure to the flag and is bucketed into the variant they saw
/// then; a user counts as converted only if they fired the goal event at or
/// after that first-exposure timestamp (so a conversion always follows the
/// exposure that could have caused it).
async fn experiment_results(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((env_id, key)): Path<(String, String)>,
) -> Result<Json<ExperimentResults>, StatusCode> {
    check_env_access(&state.db, &jar, &env_id).await?;

    let exp_row = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM experiments \
         WHERE environment_id = $1::uuid AND key = $2"
    ))
    .bind(&env_id)
    .bind(&key)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error loading experiment for results");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

    let experiment = row_to_experiment(&exp_row);

    // Bucket each user on their FIRST exposure to the flag (their experiment
    // entry point, capturing the variant seen and the entry timestamp), then
    // count a conversion only when the user fired the goal at or after that
    // entry — so a conversion always follows the exposure that could have caused
    // it. One row per observed variant with exposed (distinct users) and
    // converted (distinct users who converted post-exposure).
    let rows = sqlx::query(
        "WITH assignment AS ( \
             SELECT DISTINCT ON (user_id) \
                    user_id, value AS variant, evaluated_at AS entered_at \
             FROM impressions \
             WHERE environment_id = $1::uuid AND flag_key = $2 AND user_id IS NOT NULL \
             ORDER BY user_id, evaluated_at ASC, id ASC \
         ) \
         SELECT a.variant, \
                COUNT(*) AS exposed, \
                COUNT(*) FILTER ( \
                    WHERE EXISTS ( \
                        SELECT 1 FROM events e \
                        WHERE e.environment_id = $1::uuid \
                          AND e.event_key = $3 \
                          AND e.user_id = a.user_id \
                          AND e.occurred_at >= a.entered_at \
                    ) \
                ) AS converted \
         FROM assignment a \
         GROUP BY a.variant \
         ORDER BY exposed DESC",
    )
    .bind(&env_id)
    .bind(&experiment.flag_key)
    .bind(&experiment.goal_event_key)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error computing experiment results");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    struct Raw {
        variant: String,
        exposed: i64,
        converted: i64,
    }
    let raw: Vec<Raw> = rows
        .iter()
        .map(|r| Raw {
            variant: r.get("variant"),
            exposed: r.get("exposed"),
            converted: r.get("converted"),
        })
        .collect();

    // Control = explicit control_variant if present in the data, else the
    // highest-exposure variant (rows are ordered by exposed DESC).
    let control_variant: Option<String> = experiment
        .control_variant
        .clone()
        .filter(|cv| raw.iter().any(|r| &r.variant == cv))
        .or_else(|| raw.first().map(|r| r.variant.clone()));

    let control = control_variant
        .as_ref()
        .and_then(|cv| raw.iter().find(|r| &r.variant == cv));
    let control_rate = control.map(|c| rate(c.converted, c.exposed));
    let control_n = control.map(|c| c.exposed);
    let control_conv = control.map(|c| c.converted);

    let total_exposed: i64 = raw.iter().map(|r| r.exposed).sum();
    let total_converted: i64 = raw.iter().map(|r| r.converted).sum();

    let variants: Vec<VariantResult> = raw
        .iter()
        .map(|r| {
            let conversion_rate = rate(r.converted, r.exposed);
            let is_control = control_variant.as_ref() == Some(&r.variant);

            let (uplift, z_score, p_value) = if is_control {
                (None, None, None)
            } else if let (Some(cr), Some(cn), Some(cc)) = (control_rate, control_n, control_conv) {
                let uplift = if cr > 0.0 {
                    Some((conversion_rate - cr) / cr)
                } else {
                    None
                };
                let z = two_proportion_z(r.converted, r.exposed, cc, cn);
                let p = z.map(two_sided_p_value);
                (uplift, z, p)
            } else {
                (None, None, None)
            };

            let significant = p_value.is_some_and(|p| p < 0.05);

            VariantResult {
                variant: r.variant.clone(),
                exposed: r.exposed,
                converted: r.converted,
                conversion_rate,
                is_control,
                uplift,
                z_score,
                p_value,
                significant,
            }
        })
        .collect();

    Ok(Json(ExperimentResults {
        experiment,
        control_variant,
        total_exposed,
        total_converted,
        variants,
    }))
}

// ---------------------------------------------------------------------------
// Statistics helpers
// ---------------------------------------------------------------------------

fn rate(converted: i64, exposed: i64) -> f64 {
    if exposed <= 0 {
        0.0
    } else {
        converted as f64 / exposed as f64
    }
}

/// Two-proportion z-test statistic comparing a variant `(x1/n1)` against the
/// control `(x2/n2)` using the pooled proportion. Returns `None` when either
/// sample is empty or the pooled variance is zero (test undefined).
fn two_proportion_z(x1: i64, n1: i64, x2: i64, n2: i64) -> Option<f64> {
    if n1 <= 0 || n2 <= 0 {
        return None;
    }
    let (x1, n1, x2, n2) = (x1 as f64, n1 as f64, x2 as f64, n2 as f64);
    let p1 = x1 / n1;
    let p2 = x2 / n2;
    let pooled = (x1 + x2) / (n1 + n2);
    let se = (pooled * (1.0 - pooled) * (1.0 / n1 + 1.0 / n2)).sqrt();
    if se == 0.0 || !se.is_finite() {
        return None;
    }
    Some((p1 - p2) / se)
}

/// Two-sided p-value for a standard-normal z-score.
fn two_sided_p_value(z: f64) -> f64 {
    let p = 2.0 * (1.0 - normal_cdf(z.abs()));
    p.clamp(0.0, 1.0)
}

/// Standard normal CDF via the error function.
fn normal_cdf(x: f64) -> f64 {
    0.5 * (1.0 + erf(x / std::f64::consts::SQRT_2))
}

/// Error function — Abramowitz & Stegun 7.1.26 approximation
/// (max absolute error ~1.5e-7), which is ample for a significance readout.
fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();

    let t = 1.0 / (1.0 + 0.3275911 * x);
    let y = 1.0
        - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t
            + 0.254829592)
            * t
            * (-x * x).exp();
    sign * y
}
