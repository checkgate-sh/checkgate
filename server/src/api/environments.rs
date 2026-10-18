use crate::auth::{AuthContext, get_session_claims};
use crate::state::AppState;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Environment {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub color: String,
    pub is_default: bool,
    pub created_at: String,
    pub require_approval: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateEnvironmentRequest {
    pub name: String,
    pub slug: String,
    pub color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetRequireApprovalRequest {
    pub require_approval: bool,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

fn is_valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 64
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_valid_hex_color(color: &str) -> bool {
    let Some(hex) = color.strip_prefix('#') else {
        return false;
    };
    (hex.len() == 3 || hex.len() == 6) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

// ---------------------------------------------------------------------------
// Access helpers (project-level)
// ---------------------------------------------------------------------------

/// Returns true if the caller is a workspace admin or project member.
async fn can_access_project(
    db: &sqlx::PgPool,
    jar: &AuthContext,
    project_id: &str,
) -> Result<(), StatusCode> {
    let Some(claims) = get_session_claims(jar) else {
        return Err(StatusCode::FORBIDDEN); // SDK keys are confined to the SDK routes
    };
    if claims.role == "admin" {
        return Ok(());
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM project_members pm JOIN users u ON u.id = pm.user_id \
         WHERE pm.project_id = $1::uuid AND u.email = $2)",
    )
    .bind(project_id)
    .bind(&claims.email)
    .fetch_one(db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error checking project membership");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if exists {
        Ok(())
    } else {
        warn!(email = %claims.email, project_id = %project_id, "Forbidden: not a project member");
        Err(StatusCode::FORBIDDEN)
    }
}

/// Returns true only for workspace admin or project-level admin.
async fn can_admin_project(
    db: &sqlx::PgPool,
    jar: &AuthContext,
    project_id: &str,
) -> Result<(), StatusCode> {
    let Some(claims) = get_session_claims(jar) else {
        return Err(StatusCode::FORBIDDEN); // SDK keys are confined to the SDK routes
    };
    if claims.role == "admin" {
        return Ok(());
    }
    let role: Option<String> = sqlx::query_scalar(
        "SELECT pm.role FROM project_members pm \
         JOIN users u ON u.id = pm.user_id \
         WHERE pm.project_id = $1::uuid AND u.email = $2",
    )
    .bind(project_id)
    .bind(&claims.email)
    .fetch_optional(db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error checking project admin");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    match role.as_deref() {
        Some("admin") => Ok(()),
        Some(_) => Err(StatusCode::FORBIDDEN),
        None => Err(StatusCode::FORBIDDEN),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_environments(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<Environment>>, StatusCode> {
    can_access_project(&state.db, &jar, &project_id).await?;

    let rows = sqlx::query(
        "SELECT id::text, name, slug, color, is_default, created_at::text, require_approval \
         FROM environments WHERE project_id = $1::uuid ORDER BY created_at ASC",
    )
    .bind(&project_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to list environments");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let envs = rows
        .iter()
        .map(|r| Environment {
            id: r.get("id"),
            name: r.get("name"),
            slug: r.get("slug"),
            color: r.get("color"),
            is_default: r.get("is_default"),
            created_at: r.get("created_at"),
            require_approval: r.get("require_approval"),
        })
        .collect();

    Ok(Json(envs))
}

async fn create_environment(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(project_id): Path<String>,
    Json(req): Json<CreateEnvironmentRequest>,
) -> Result<Json<Environment>, StatusCode> {
    can_admin_project(&state.db, &jar, &project_id).await?;

    let name = req.name.trim().to_string();
    let slug = req.slug.trim().to_lowercase();
    let color = req.color.unwrap_or_else(|| "#6366f1".to_string());

    if name.is_empty() || name.len() > 100 {
        warn!("Rejected create_environment: name empty or too long");
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    if !is_valid_slug(&slug) {
        warn!(slug = %slug, "Rejected create_environment: invalid slug");
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    if !is_valid_hex_color(&color) {
        warn!(color = %color, "Rejected create_environment: invalid hex color");
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let row = sqlx::query(
        "INSERT INTO environments (name, slug, color, project_id) VALUES ($1, $2, $3, $4::uuid) \
         RETURNING id::text, name, slug, color, is_default, created_at::text, require_approval",
    )
    .bind(&name)
    .bind(&slug)
    .bind(&color)
    .bind(&project_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to create environment");
        if e.to_string().to_lowercase().contains("unique") {
            StatusCode::CONFLICT
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    })?;

    let env = Environment {
        id: row.get("id"),
        name: row.get("name"),
        slug: row.get("slug"),
        color: row.get("color"),
        is_default: row.get("is_default"),
        created_at: row.get("created_at"),
        require_approval: row.get("require_approval"),
    };

    info!(slug = %env.slug, project_id = %project_id, "Environment created");
    Ok(Json(env))
}

async fn delete_environment(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((project_id, env_id)): Path<(String, String)>,
) -> Result<StatusCode, StatusCode> {
    can_admin_project(&state.db, &jar, &project_id).await?;

    let row = sqlx::query(
        "SELECT is_default FROM environments WHERE id = $1::uuid AND project_id = $2::uuid",
    )
    .bind(&env_id)
    .bind(&project_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to fetch environment for delete");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

    let is_default: bool = row.get("is_default");
    if is_default {
        warn!(id = %env_id, "Rejected delete: cannot delete the default environment");
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    // Prevent deleting the last environment in the project.
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM environments WHERE project_id = $1::uuid")
            .bind(&project_id)
            .fetch_one(&state.db)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to count environments");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

    if count <= 1 {
        warn!("Rejected delete: cannot delete the last environment in project");
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let result =
        sqlx::query("DELETE FROM environments WHERE id = $1::uuid AND project_id = $2::uuid")
            .bind(&env_id)
            .bind(&project_id)
            .execute(&state.db)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to delete environment");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

    if result.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    info!(id = %env_id, project_id = %project_id, "Environment deleted");
    Ok(StatusCode::NO_CONTENT)
}

async fn set_default_environment(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((project_id, env_id)): Path<(String, String)>,
) -> Result<Json<Environment>, StatusCode> {
    can_admin_project(&state.db, &jar, &project_id).await?;

    // Verify env belongs to this project.
    let belongs: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM environments WHERE id = $1::uuid AND project_id = $2::uuid)",
    )
    .bind(&env_id)
    .bind(&project_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error checking environment ownership");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if !belongs {
        return Err(StatusCode::NOT_FOUND);
    }

    let mut db_tx = state.db.begin().await.map_err(|e| {
        error!(error = %e, "Failed to begin transaction");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Serialize default changes within the project, so two concurrent requests
    // cannot both clear and then both set.
    sqlx::query("SELECT 1 FROM projects WHERE id = $1::uuid FOR UPDATE")
        .bind(&project_id)
        .execute(&mut *db_tx)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to lock project");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Clear the current default first: the unique index on (project_id) WHERE
    // is_default is checked per row, so setting the new default while the old
    // one is still marked would fail. Other transactions never see the gap.
    sqlx::query(
        "UPDATE environments SET is_default = false \
         WHERE project_id = $1::uuid AND id != $2::uuid AND is_default",
    )
    .bind(&project_id)
    .bind(&env_id)
    .execute(&mut *db_tx)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to clear other default environments");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let row = sqlx::query(
        "UPDATE environments SET is_default = true WHERE id = $1::uuid \
         RETURNING id::text, name, slug, color, is_default, created_at::text, require_approval",
    )
    .bind(&env_id)
    .fetch_optional(&mut *db_tx)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to set default environment");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

    db_tx.commit().await.map_err(|e| {
        error!(error = %e, "Failed to commit set-default transaction");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let env = Environment {
        id: row.get("id"),
        name: row.get("name"),
        slug: row.get("slug"),
        color: row.get("color"),
        is_default: row.get("is_default"),
        created_at: row.get("created_at"),
        require_approval: row.get("require_approval"),
    };

    info!(slug = %env.slug, project_id = %project_id, "Default environment updated");
    Ok(Json(env))
}

/// POST /api/projects/:project_id/environments/:env_id/require-approval
///
/// Toggles whether flag PATCHes in this environment must go through a change
/// request instead of applying immediately. Admin only, like other
/// environment-level settings — this is a guard rail, not a per-user preference.
async fn set_require_approval(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((project_id, env_id)): Path<(String, String)>,
    Json(req): Json<SetRequireApprovalRequest>,
) -> Result<Json<Environment>, StatusCode> {
    can_admin_project(&state.db, &jar, &project_id).await?;

    let row = sqlx::query(
        "UPDATE environments SET require_approval = $1 \
         WHERE id = $2::uuid AND project_id = $3::uuid \
         RETURNING id::text, name, slug, color, is_default, created_at::text, require_approval",
    )
    .bind(req.require_approval)
    .bind(&env_id)
    .bind(&project_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to set require_approval");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

    let env = Environment {
        id: row.get("id"),
        name: row.get("name"),
        slug: row.get("slug"),
        color: row.get("color"),
        is_default: row.get("is_default"),
        created_at: row.get("created_at"),
        require_approval: row.get("require_approval"),
    };

    info!(slug = %env.slug, project_id = %project_id, require_approval = req.require_approval, "Environment approval requirement updated");
    Ok(Json(env))
}

// ---------------------------------------------------------------------------
// Routers  (nested under /projects/{project_id})
// ---------------------------------------------------------------------------

pub fn read_router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project_id}/environments",
        get(list_environments),
    )
}

pub fn write_router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project_id}/environments",
            post(create_environment),
        )
        .route(
            "/projects/{project_id}/environments/{env_id}",
            delete(delete_environment),
        )
        .route(
            "/projects/{project_id}/environments/{env_id}/default",
            post(set_default_environment),
        )
        .route(
            "/projects/{project_id}/environments/{env_id}/require-approval",
            post(set_require_approval),
        )
}
