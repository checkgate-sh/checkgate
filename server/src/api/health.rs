//! SDK health: the live SSE connections held by **this server instance**.
//!
//! Connections are tracked in process memory (`AppState::connected_clients`),
//! so behind a load balancer each instance reports only its own clients.
//!
//! Visibility follows project RBAC: workspace admins see every connection;
//! anyone else sees only connections of environments in projects they are a
//! member of, with the client address reduced to its network prefix.
//! Connections without an environment (dashboard sessions, the legacy
//! `SDK_KEY`) are visible to workspace admins only.

use crate::auth::{AuthContext, get_session_claims};
use crate::state::{AppState, ConnectedClient};
use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use ipnet::IpNet;
use serde::Serialize;
use std::collections::HashSet;
use std::net::IpAddr;
use tracing::error;

#[derive(Debug, Serialize)]
pub struct ConnectedClientView {
    pub connection_id: String,
    pub environment_id: Option<String>,
    pub sdk_key_name: Option<String>,
    /// Full address for workspace admins; a network prefix such as
    /// `203.0.113.0/24` for everyone else.
    pub client_ip: String,
    pub connected_at: i64,
    /// Highest flag revision this instance has written to the stream;
    /// `None` until the bootstrap replay was sent. Delivered, not
    /// necessarily applied: the SDK's own `getHealth()` says what it holds.
    pub last_delivered_revision: Option<i64>,
}

/// Server-observed convergence of one environment's streams on this instance.
#[derive(Debug, Serialize)]
pub struct EnvironmentHealthView {
    pub environment_id: String,
    /// Open streams on this instance.
    pub connections: usize,
    /// Highest committed revision of the environment's flags (deletions
    /// included); `None` for an environment that never had a flag.
    pub latest_revision: Option<i64>,
    /// Streams whose last delivered revision is below `latest_revision`:
    /// a change is committed but not yet written to them (in flight, or
    /// lost and awaiting reconciliation).
    pub connections_behind: usize,
    /// When the oldest open stream connected (Unix seconds).
    pub oldest_connected_at: Option<i64>,
}

pub fn read_router() -> Router<AppState> {
    Router::new()
        .route("/health/connections", get(list_connections))
        .route("/health/environments", get(environment_health))
}

/// IPv4 /24 or IPv6 /48: enough to tell sites apart, not to single out a host.
fn masked(ip: IpAddr) -> String {
    let prefix = if ip.is_ipv4() { 24 } else { 48 };
    IpNet::new(ip, prefix)
        .map(|net| net.trunc().to_string())
        .unwrap_or_default()
}

fn view(c: &ConnectedClient, full_ip: bool) -> ConnectedClientView {
    ConnectedClientView {
        connection_id: c.connection_id.clone(),
        environment_id: c.environment_id.clone(),
        sdk_key_name: c.sdk_key_name.clone(),
        client_ip: if full_ip {
            c.client_ip.to_string()
        } else {
            masked(c.client_ip)
        },
        connected_at: c.connected_at,
        last_delivered_revision: delivered(c),
    }
}

fn delivered(c: &ConnectedClient) -> Option<i64> {
    let revision = c
        .last_delivered_revision
        .load(std::sync::atomic::Ordering::Relaxed);
    (revision >= 0).then_some(revision)
}

/// Environment ids the caller may see, or `None` for "all" (workspace admin).
async fn visible_environments(
    state: &AppState,
    ctx: &AuthContext,
) -> Result<(bool, Option<HashSet<String>>), StatusCode> {
    // Sessions and PATs (either scope — this is a read) carry a user; SDK keys
    // never reach these routes (`sdk_key_route_allowed`), but refuse them anyway.
    let claims = get_session_claims(ctx).ok_or(StatusCode::FORBIDDEN)?;
    if claims.role == "admin" {
        return Ok((true, None));
    }
    let envs: Vec<String> = sqlx::query_scalar(
        "SELECT e.id::text FROM environments e \
         JOIN project_members pm ON pm.project_id = e.project_id \
         WHERE pm.user_id = $1",
    )
    .bind(claims.user_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error resolving SDK health visibility");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok((false, Some(envs.into_iter().collect())))
}

/// GET /api/health/environments — per environment with open streams on this
/// instance: how many, the environment's latest committed revision, and how
/// many streams have not yet been sent it. Counts only; no addresses.
async fn environment_health(
    State(state): State<AppState>,
    ctx: AuthContext,
) -> Result<Json<Vec<EnvironmentHealthView>>, StatusCode> {
    let (_, visible) = visible_environments(&state, &ctx).await?;
    // (connections, delivered revisions, oldest connected_at) per environment.
    let mut by_env: std::collections::BTreeMap<String, (usize, Vec<Option<i64>>, i64)> =
        std::collections::BTreeMap::new();
    for entry in state.connected_clients.iter() {
        let c = entry.value();
        let Some(env) = c.environment_id.as_ref() else {
            continue;
        };
        if visible.as_ref().is_some_and(|v| !v.contains(env)) {
            continue;
        }
        let slot = by_env
            .entry(env.clone())
            .or_insert((0, Vec::new(), i64::MAX));
        slot.0 = slot.0.saturating_add(1);
        slot.1.push(delivered(c));
        slot.2 = slot.2.min(c.connected_at);
    }
    let ids: Vec<String> = by_env.keys().cloned().collect();
    let latest: Vec<(String, Option<i64>)> = if ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(
            "SELECT e.id::text, GREATEST( \
                 (SELECT MAX(revision) FROM flags f WHERE f.environment_id = e.id), \
                 (SELECT MAX(revision) FROM flag_tombstones t WHERE t.environment_id = e.id)) \
             FROM environments e WHERE e.id::text = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&state.db)
        .await
        .map_err(|e| {
            error!(error = %e, "DB error reading environment revisions");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
    };
    let latest: std::collections::HashMap<String, Option<i64>> = latest.into_iter().collect();
    Ok(Json(
        by_env
            .into_iter()
            .map(|(environment_id, (connections, delivered, oldest))| {
                let latest_revision = latest.get(&environment_id).copied().flatten();
                let connections_behind = match latest_revision {
                    Some(l) => delivered.iter().filter(|d| d.is_none_or(|d| d < l)).count(),
                    None => 0,
                };
                EnvironmentHealthView {
                    environment_id,
                    connections,
                    latest_revision,
                    connections_behind,
                    oldest_connected_at: (oldest != i64::MAX).then_some(oldest),
                }
            })
            .collect(),
    ))
}

async fn list_connections(
    State(state): State<AppState>,
    ctx: AuthContext,
) -> Result<Json<Vec<ConnectedClientView>>, StatusCode> {
    let (is_admin, visible_envs) = visible_environments(&state, &ctx).await?;

    let mut clients: Vec<ConnectedClientView> = state
        .connected_clients
        .iter()
        .filter(|entry| match &visible_envs {
            None => true,
            Some(envs) => entry
                .value()
                .environment_id
                .as_ref()
                .is_some_and(|env| envs.contains(env)),
        })
        .map(|entry| view(entry.value(), is_admin))
        .collect();

    // Stable ordering so the dashboard doesn't jump around.
    clients.sort_by(|a, b| {
        a.connected_at
            .cmp(&b.connected_at)
            .then_with(|| a.connection_id.cmp(&b.connection_id))
    });
    Ok(Json(clients))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_to_network_prefix() {
        assert_eq!(masked("203.0.113.77".parse().unwrap()), "203.0.113.0/24");
        assert_eq!(
            masked("2001:db8:abcd:12::1".parse().unwrap()),
            "2001:db8:abcd::/48"
        );
        assert_eq!(masked("127.0.0.1".parse().unwrap()), "127.0.0.0/24");
    }
}
