use crate::auth::{AuthContext, get_session_claims};
use crate::state::SdkKeyEntry;
use crate::state::{AppState, ConnectedClient};
use axum::http::{HeaderName, HeaderValue};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
};
use dashmap::DashMap;
use futures_util::stream::Stream;
use rand::RngExt as _;
use sqlx::Row;
use std::collections::HashSet;
use std::{convert::Infallible, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::broadcast;
use tracing::{info, warn};

/// Removes the client from the health dashboard tracking map when dropped.
struct ConnectionGuard {
    clients: Arc<DashMap<String, ConnectedClient>>,
    connection_id: String,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.clients.remove(&self.connection_id);
        info!(connection_id = %self.connection_id, "SSE client unregistered from health tracking");
    }
}

fn random_connection_id() -> String {
    let mut bytes = [0u8; 8];
    rand::rng().fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[allow(
    clippy::cast_possible_wrap,
    reason = "seconds since the Unix epoch; wrapping i64 would require a clock set beyond the year 292 billion"
)]
pub async fn sse_handler(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    ctx: AuthContext,
    req: axum::http::Request<axum::body::Body>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let client_ip = addr.ip();

    let sdk_key = req.extensions().get::<SdkKeyEntry>();
    let environment_id = sdk_key.and_then(|key| key.environment_id.clone());
    let sdk_key_name = sdk_key.map(|key| key.name.clone());
    let claims = get_session_claims(&ctx);
    let env_ids: Vec<String> = if let Some(env_id) = &environment_id {
        vec![env_id.clone()]
    } else if let Some(claims) = &claims {
        sqlx::query_scalar(
            "SELECT e.id::text FROM environments e WHERE $2 = 'admin' OR EXISTS( \
             SELECT 1 FROM project_members pm WHERE pm.project_id = e.project_id AND pm.user_id = $1)",
        ).bind(claims.user_id).bind(&claims.role).fetch_all(&state.db).await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    } else {
        // Only validated legacy SDK_KEY credentials have unscoped machine access.
        sqlx::query_scalar("SELECT id::text FROM environments")
            .fetch_all(&state.db)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    let allowed_envs: HashSet<String> = env_ids.iter().cloned().collect();
    // Read bootstrap before opening the stream. A query failure must not signal an empty, ready store.
    let rx = state.flag_tx.subscribe();
    let mut bootstrap = Vec::new();
    for env_id in &env_ids {
        let segments = crate::api::segments::load_env_segments(env_id, &state.db)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let rows =
            sqlx::query("SELECT data FROM flags WHERE environment_id = $1::uuid ORDER BY key")
                .bind(env_id)
                .fetch_all(&state.db)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        for row in rows {
            let data: serde_json::Value = row
                .try_get("data")
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let flag =
                serde_json::from_value(data).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let flag = crate::api::segments::expand_flag_with_segments(flag, &segments);
            bootstrap.push(
                serde_json::json!({"type":"UPSERT", "env_id":env_id, "flag":flag}).to_string(),
            );
        }
    }

    // Register this connection for the SDK health dashboard.
    let connection_id = random_connection_id();
    let connected_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    state.connected_clients.insert(
        connection_id.clone(),
        ConnectedClient {
            connection_id: connection_id.clone(),
            environment_id: environment_id.clone(),
            sdk_key_name: sdk_key_name.clone(),
            client_ip: client_ip.to_string(),
            connected_at,
        },
    );

    info!(
        client_ip = %client_ip,
        connection_id = %connection_id,
        environment_id = ?environment_id,
        sdk_key_name = ?sdk_key_name,
        "SSE client connected"
    );

    let mut rx = rx;
    let connected_clients = Arc::clone(&state.connected_clients);

    let stream = async_stream::stream! {
        // Guard ensures the client is removed when the stream drops (on disconnect or break).
        let _guard = ConnectionGuard { clients: connected_clients, connection_id: connection_id.clone() };

        // Announce the resolved environment so SDK clients know where to report
        // impressions (POST /api/environments/{environment_id}/impressions).
        // `null` for session-auth (dashboard) connections, which do not report.
        let connected_payload = serde_json::json!({
            "environment_id": environment_id.clone(),
        })
        .to_string();
        yield Ok(Event::default().event("connected").data(connected_payload));

        let flag_count = bootstrap.len();
        for payload in bootstrap {
            yield Ok(Event::default().event("update").data(payload));
        }

        info!(
            client_ip = %client_ip,
            flags_sent = flag_count,
            "SSE bootstrap complete"
        );

        // Signal end of the initial state dump so SDK clients can resolve connect()
        // deterministically (instead of guessing when bootstrap finished). Sent on
        // every (re)connect after the full flag set has been replayed.
        yield Ok(Event::default().event("ready").data(flag_count.to_string()));

        // Stream live deltas, filtering by environment_id when present.
        loop {
            match rx.recv().await {
                Ok(payload) => {
                    let should_forward = serde_json::from_str::<serde_json::Value>(&payload)
                        .ok().and_then(|v| v.get("env_id").and_then(|e| e.as_str())
                            .map(|env| allowed_envs.contains(env))).unwrap_or(false);
                    if should_forward {
                        yield Ok(Event::default().event("update").data(payload));
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(
                        client_ip = %client_ip,
                        missed_messages = n,
                        "SSE client lagged — closing connection to trigger reconnect"
                    );
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    info!(client_ip = %client_ip, "SSE broadcast channel closed");
                    break;
                }
            }
        }

        info!(client_ip = %client_ip, "SSE client disconnected");
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive-text"),
    ))
}

/// GET /flags/snapshot — SDK-key-authed, segment-expanding full flag snapshot.
///
/// SDK clients poll this as a fallback when the SSE stream at `/stream` cannot be
/// established (e.g. a proxy or firewall blocking long-lived connections, or after
/// repeated reconnect failures). Unlike `GET /api/environments/{env_id}/flags`
/// (session-cookie only, no segment expansion — see `server/src/api/flags.rs`),
/// this route is keyed by SDK key alone, exactly like `/stream`, and returns flags
/// with segment references already expanded so the response is ready to evaluate
/// against directly, matching what SSE bootstrap sends.
///
/// Returns 400 if called with session-cookie auth (dashboard) rather than an SDK
/// key — the dashboard isn't scoped to a single environment the way an SDK key is.
pub async fn flags_snapshot_handler(
    State(state): State<AppState>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<
    (
        axum::http::HeaderMap,
        Json<Vec<checkgate_core::evaluator::Flag>>,
    ),
    StatusCode,
> {
    let env_id = req
        .extensions()
        .get::<SdkKeyEntry>()
        .and_then(|key| key.environment_id.clone())
        .ok_or(StatusCode::BAD_REQUEST)?;

    let segment_map = crate::api::segments::load_env_segments(&env_id, &state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let rows =
        sqlx::query("SELECT data FROM flags WHERE environment_id = $1::uuid ORDER BY key ASC")
            .bind(&env_id)
            .fetch_all(&state.db)
            .await
            .map_err(|e| {
                warn!(error = %e, "flags_snapshot: DB query failed");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

    let flags: Vec<checkgate_core::evaluator::Flag> = rows
        .iter()
        .filter_map(|row| {
            let v: serde_json::Value = row.try_get("data").ok()?;
            let flag = serde_json::from_value::<checkgate_core::evaluator::Flag>(v).ok()?;
            Some(crate::api::segments::expand_flag_with_segments(
                flag,
                &segment_map,
            ))
        })
        .collect();

    info!(
        env_id = %env_id,
        flag_count = flags.len(),
        "Flags snapshot served (poll fallback)"
    );

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        HeaderName::from_static("x-checkgate-environment-id"),
        HeaderValue::from_str(&env_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    Ok((headers, Json(flags)))
}
