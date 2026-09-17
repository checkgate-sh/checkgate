use crate::auth::{AuthContext, get_session_claims};
use crate::state::AppState;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, patch, post},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub environment_count: i64,
    pub member_count: i64,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct ProjectMemberInfo {
    pub user_id: i64,
    pub name: String,
    pub email: String,
    pub role: String,
}

#[derive(Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
}

#[derive(Deserialize)]
pub struct RenameProjectRequest {
    pub name: String,
}

#[derive(Deserialize)]
pub struct AddMemberRequest {
    pub user_id: i64,
    pub role: String,
}

#[derive(Deserialize)]
pub struct UpdateMemberRoleRequest {
    pub role: String,
}

// ---------------------------------------------------------------------------
// Access helpers
// ---------------------------------------------------------------------------

/// Returns the effective role the caller has within a project.
/// - Workspace admin → "admin" (full access, no membership row needed)
/// - SDK key auth (no session cookie) → "admin" (machine credentials)
/// - Session with project membership → the membership role
/// - Session with no membership → Err(403)
async fn project_role(
    db: &sqlx::PgPool,
    jar: &AuthContext,
    project_id: &str,
) -> Result<String, StatusCode> {
    let Some(claims) = get_session_claims(jar) else {
        // SDK key auth — admin-equivalent for all projects.
        return Ok("admin".into());
    };

    if claims.role == "admin" {
        return Ok("admin".into());
    }

    // Look up project membership for this user.
    let row = sqlx::query(
        "SELECT pm.role FROM project_members pm \
         JOIN users u ON u.id = pm.user_id \
         WHERE pm.project_id = $1::uuid AND u.email = $2",
    )
    .bind(project_id)
    .bind(&claims.email)
    .fetch_optional(db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error checking project membership");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    match row {
        Some(r) => Ok(r.get::<String, _>("role")),
        None => {
            warn!(email = %claims.email, project_id = %project_id, "Forbidden: not a project member");
            Err(StatusCode::FORBIDDEN)
        }
    }
}

fn assert_admin(role: &str) -> Result<(), StatusCode> {
    if role == "admin" {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

// ---------------------------------------------------------------------------
// Slug helpers
// ---------------------------------------------------------------------------

fn slugify(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

// ---------------------------------------------------------------------------
// Handlers — project CRUD
// ---------------------------------------------------------------------------

async fn list_projects(
    State(state): State<AppState>,
    jar: AuthContext,
) -> Result<Json<Vec<ProjectSummary>>, StatusCode> {
    let is_admin_or_sdk = match get_session_claims(&jar) {
        None => true,
        Some(ref c) if c.role == "admin" => true,
        _ => false,
    };

    let rows = if is_admin_or_sdk {
        sqlx::query(
            "SELECT p.id::text, p.name, p.slug, p.created_at::text, \
             (SELECT COUNT(*) FROM environments e WHERE e.project_id = p.id) AS environment_count, \
             (SELECT COUNT(*) FROM project_members pm WHERE pm.project_id = p.id) AS member_count \
             FROM projects p ORDER BY p.created_at ASC",
        )
        .fetch_all(&state.db)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to list all projects");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
    } else {
        // Non-admin: only projects where the user has membership.
        let email = get_session_claims(&jar)
            .map(|c| c.email)
            .unwrap_or_default();
        sqlx::query(
            "SELECT p.id::text, p.name, p.slug, p.created_at::text, \
             (SELECT COUNT(*) FROM environments e WHERE e.project_id = p.id) AS environment_count, \
             (SELECT COUNT(*) FROM project_members pm2 WHERE pm2.project_id = p.id) AS member_count \
             FROM projects p \
             JOIN project_members pm ON pm.project_id = p.id \
             JOIN users u ON u.id = pm.user_id \
             WHERE u.email = $1 \
             ORDER BY p.created_at ASC",
        )
        .bind(&email)
        .fetch_all(&state.db)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to list user projects");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
    };

    let projects = rows
        .iter()
        .map(|r| ProjectSummary {
            id: r.get("id"),
            name: r.get("name"),
            slug: r.get("slug"),
            environment_count: r.get("environment_count"),
            member_count: r.get("member_count"),
            created_at: r.get("created_at"),
        })
        .collect();

    Ok(Json(projects))
}

async fn create_project(
    State(state): State<AppState>,
    jar: AuthContext,
    Json(req): Json<CreateProjectRequest>,
) -> Result<Json<Project>, StatusCode> {
    // Only workspace admins can create projects.
    let claims = get_session_claims(&jar);
    let is_admin = claims.as_ref().map(|c| c.role == "admin").unwrap_or(true);
    if !is_admin {
        return Err(StatusCode::FORBIDDEN);
    }

    let name = req.name.trim().to_string();
    if name.is_empty() || name.len() > 100 {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let slug = slugify(&name);
    if slug.is_empty() {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let row = sqlx::query(
        "INSERT INTO projects (name, slug) VALUES ($1, $2) \
         RETURNING id::text, name, slug, created_at::text",
    )
    .bind(&name)
    .bind(&slug)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to create project");
        if e.to_string().to_lowercase().contains("unique") {
            StatusCode::CONFLICT
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    })?;

    let project = Project {
        id: row.get("id"),
        name: row.get("name"),
        slug: row.get("slug"),
        created_at: row.get("created_at"),
    };

    info!(name = %project.name, slug = %project.slug, "Project created");
    Ok(Json(project))
}

async fn get_project(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(project_id): Path<String>,
) -> Result<Json<Project>, StatusCode> {
    project_role(&state.db, &jar, &project_id).await?;

    let row = sqlx::query(
        "SELECT id::text, name, slug, created_at::text FROM projects WHERE id = $1::uuid",
    )
    .bind(&project_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to fetch project");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(Project {
        id: row.get("id"),
        name: row.get("name"),
        slug: row.get("slug"),
        created_at: row.get("created_at"),
    }))
}

async fn rename_project(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(project_id): Path<String>,
    Json(req): Json<RenameProjectRequest>,
) -> Result<Json<Project>, StatusCode> {
    let role = project_role(&state.db, &jar, &project_id).await?;
    assert_admin(&role)?;

    let name = req.name.trim().to_string();
    if name.is_empty() || name.len() > 100 {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let row = sqlx::query(
        "UPDATE projects SET name = $1 WHERE id = $2::uuid \
         RETURNING id::text, name, slug, created_at::text",
    )
    .bind(&name)
    .bind(&project_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to rename project");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

    info!(project_id = %project_id, name = %name, "Project renamed");
    Ok(Json(Project {
        id: row.get("id"),
        name: row.get("name"),
        slug: row.get("slug"),
        created_at: row.get("created_at"),
    }))
}

async fn delete_project(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(project_id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let role = project_role(&state.db, &jar, &project_id).await?;
    assert_admin(&role)?;

    // Wrap count + delete in a transaction with FOR UPDATE so that two concurrent
    // delete requests cannot both pass the "last project" check simultaneously.
    let mut tx = state.db.begin().await.map_err(|e| {
        error!(error = %e, "Failed to begin delete transaction");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (SELECT id FROM projects ORDER BY id FOR UPDATE) AS locked",
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to count projects");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if count <= 1 {
        let _ = tx.rollback().await;
        warn!(project_id = %project_id, "Rejected delete: cannot delete the last project");
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let result = sqlx::query("DELETE FROM projects WHERE id = $1::uuid")
        .bind(&project_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to delete project");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if result.rows_affected() == 0 {
        let _ = tx.rollback().await;
        return Err(StatusCode::NOT_FOUND);
    }

    tx.commit().await.map_err(|e| {
        error!(error = %e, "Transaction commit failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    info!(project_id = %project_id, "Project deleted");
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Handlers — members
// ---------------------------------------------------------------------------

async fn list_members(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<ProjectMemberInfo>>, StatusCode> {
    project_role(&state.db, &jar, &project_id).await?;

    let rows = sqlx::query(
        "SELECT u.id, u.name, u.email, pm.role \
         FROM project_members pm \
         JOIN users u ON u.id = pm.user_id \
         WHERE pm.project_id = $1::uuid \
         ORDER BY u.name ASC",
    )
    .bind(&project_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to list project members");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let members = rows
        .iter()
        .map(|r| ProjectMemberInfo {
            user_id: r.get("id"),
            name: r.get("name"),
            email: r.get("email"),
            role: r.get("role"),
        })
        .collect();

    Ok(Json(members))
}

async fn add_member(
    State(state): State<AppState>,
    jar: AuthContext,
    Path(project_id): Path<String>,
    Json(req): Json<AddMemberRequest>,
) -> Result<Json<ProjectMemberInfo>, StatusCode> {
    let role = project_role(&state.db, &jar, &project_id).await?;
    assert_admin(&role)?;

    if !matches!(req.role.as_str(), "admin" | "editor" | "viewer") {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    // Verify the user exists.
    let user_row = sqlx::query("SELECT id, name, email FROM users WHERE id = $1")
        .bind(req.user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to fetch user for membership");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    sqlx::query(
        "INSERT INTO project_members (project_id, user_id, role) VALUES ($1::uuid, $2, $3) \
         ON CONFLICT (project_id, user_id) DO UPDATE SET role = EXCLUDED.role",
    )
    .bind(&project_id)
    .bind(req.user_id)
    .bind(&req.role)
    .execute(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to add project member");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    info!(project_id = %project_id, user_id = req.user_id, role = %req.role, "Member added to project");
    Ok(Json(ProjectMemberInfo {
        user_id: user_row.get("id"),
        name: user_row.get("name"),
        email: user_row.get("email"),
        role: req.role,
    }))
}

async fn update_member_role(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((project_id, user_id)): Path<(String, i64)>,
    Json(req): Json<UpdateMemberRoleRequest>,
) -> Result<StatusCode, StatusCode> {
    let role = project_role(&state.db, &jar, &project_id).await?;
    assert_admin(&role)?;

    if !matches!(req.role.as_str(), "admin" | "editor" | "viewer") {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let result = sqlx::query(
        "UPDATE project_members SET role = $1 WHERE project_id = $2::uuid AND user_id = $3",
    )
    .bind(&req.role)
    .bind(&project_id)
    .bind(user_id)
    .execute(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "Failed to update member role");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if result.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    info!(project_id = %project_id, user_id = %user_id, role = %req.role, "Member role updated");
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_member(
    State(state): State<AppState>,
    jar: AuthContext,
    Path((project_id, user_id)): Path<(String, i64)>,
) -> Result<StatusCode, StatusCode> {
    let role = project_role(&state.db, &jar, &project_id).await?;
    assert_admin(&role)?;

    let result =
        sqlx::query("DELETE FROM project_members WHERE project_id = $1::uuid AND user_id = $2")
            .bind(&project_id)
            .bind(user_id)
            .execute(&state.db)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to remove project member");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

    if result.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    info!(project_id = %project_id, user_id = %user_id, "Member removed from project");
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Routers
// ---------------------------------------------------------------------------

/// Read routes — any authenticated user (filtered by membership for non-admins).
pub fn read_router() -> Router<AppState> {
    Router::new()
        .route("/projects", get(list_projects))
        .route("/projects/{id}", get(get_project))
        .route("/projects/{id}/members", get(list_members))
}

/// Write routes — admin workspace role or project-level admin.
pub fn write_router() -> Router<AppState> {
    Router::new()
        .route("/projects", post(create_project))
        .route("/projects/{id}", patch(rename_project))
        .route("/projects/{id}", delete(delete_project))
        .route("/projects/{id}/members", post(add_member))
        .route(
            "/projects/{id}/members/{user_id}",
            patch(update_member_role),
        )
        .route("/projects/{id}/members/{user_id}", delete(remove_member))
}
