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
}

pub fn read_router() -> Router<AppState> {
    Router::new().route("/health/connections", get(list_connections))
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
    }
}

async fn list_connections(
    State(state): State<AppState>,
    ctx: AuthContext,
) -> Result<Json<Vec<ConnectedClientView>>, StatusCode> {
    // Sessions and PATs (either scope — this is a read) carry a user; SDK keys
    // never reach this route (`sdk_key_route_allowed`), but refuse them anyway.
    let claims = get_session_claims(&ctx).ok_or(StatusCode::FORBIDDEN)?;
    let is_admin = claims.role == "admin";

    let visible_envs: Option<HashSet<String>> = if is_admin {
        None
    } else {
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
        Some(envs.into_iter().collect())
    };

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
