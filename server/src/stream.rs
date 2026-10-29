use crate::auth::{AuthContext, get_session_claims};
use crate::client_ip::ClientIp;
use crate::metrics::{DisconnectReason, Metrics, RejectReason};
use crate::state::SdkKeyEntry;
use crate::state::{AppState, ConnectedClient};
use crate::sync::EventKind;
use axum::http::{HeaderName, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
};
use dashmap::DashMap;
use futures_util::stream::Stream;
use rand::RngExt as _;
use sqlx::Row;
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast};
use tracing::{info, warn};

/// Admission control for SSE streams and full-environment reads.
///
/// - At most `SSE_MAX_CONNECTIONS` streams are open per instance (default
///   10 000; `0` = unlimited). Beyond that, `/stream` answers 503 with
///   `Retry-After`; SDKs back off and, after repeated failures, poll
///   `/flags/snapshot` instead.
/// - At most `SSE_BOOTSTRAP_CONCURRENCY` bootstraps and snapshots read
///   PostgreSQL at once (default: half of `DB_MAX_CONNECTIONS`, at least 1),
///   so a reconnect wave cannot take every pool connection from the REST API.
///   A request that waits longer than `SSE_BOOTSTRAP_QUEUE_TIMEOUT_SECS`
///   (default 10) for a slot gets 503 with `Retry-After`.
pub struct SseLimits {
    max_connections: Option<usize>,
    active: AtomicUsize,
    bootstrap: Arc<Semaphore>,
    bootstrap_wait: Duration,
}

impl SseLimits {
    pub fn from_env(db_max_connections: u32) -> Result<Self, String> {
        Self::from_lookup(db_max_connections, |n| std::env::var(n).ok())
    }

    fn from_lookup(
        db_max_connections: u32,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let read = |name: &str, default: u64| -> Result<u64, String> {
            match lookup(name) {
                None => Ok(default),
                Some(v) => v
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| format!("{name} must be a non-negative integer, got {v:?}")),
            }
        };
        let max = read("SSE_MAX_CONNECTIONS", 10_000)?;
        let concurrency = read(
            "SSE_BOOTSTRAP_CONCURRENCY",
            u64::from((db_max_connections / 2).max(1)),
        )?;
        if concurrency == 0 {
            return Err("SSE_BOOTSTRAP_CONCURRENCY must be at least 1".into());
        }
        let wait = read("SSE_BOOTSTRAP_QUEUE_TIMEOUT_SECS", 10)?;
        let to_usize = |v: u64| usize::try_from(v).unwrap_or(usize::MAX);
        Ok(Self {
            max_connections: (max > 0).then(|| to_usize(max)),
            active: AtomicUsize::new(0),
            bootstrap: Arc::new(Semaphore::new(
                to_usize(concurrency).min(Semaphore::MAX_PERMITS),
            )),
            bootstrap_wait: Duration::from_secs(wait),
        })
    }

    pub fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    /// Reserves a stream slot, or `None` at capacity.
    fn try_reserve(self: &Arc<Self>) -> Option<StreamSlot> {
        let limits = Arc::clone(self);
        self.active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                match self.max_connections {
                    Some(max) if n >= max => None,
                    _ => n.checked_add(1),
                }
            })
            .ok()
            .map(|_| StreamSlot { limits })
    }

    /// Waits (bounded) for a bootstrap slot.
    async fn bootstrap_permit(&self) -> Option<OwnedSemaphorePermit> {
        tokio::time::timeout(
            self.bootstrap_wait,
            Arc::clone(&self.bootstrap).acquire_owned(),
        )
        .await
        .ok()
        .and_then(Result::ok)
    }
}

/// One reserved stream; released on drop.
struct StreamSlot {
    limits: Arc<SseLimits>,
}

impl Drop for StreamSlot {
    fn drop(&mut self) {
        self.limits.active.fetch_sub(1, Ordering::AcqRel);
    }
}

/// 503 with a `Retry-After` jittered over a few seconds, so refused clients
/// do not come back in lockstep.
fn unavailable(message: &str) -> Response {
    let retry_after = rand::rng().random_range(2u32..=10);
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({ "error": message, "retry_after_seconds": retry_after })),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(retry_after));
    response
}

/// Unregisters the client from the health dashboard, releases its stream
/// slot and records why it ended — when the stream is dropped, whether or
/// not it was ever polled.
struct ConnectionGuard {
    clients: Arc<DashMap<String, ConnectedClient>>,
    connection_id: String,
    metrics: Arc<Metrics>,
    reason: DisconnectReason,
    _slot: StreamSlot,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.clients.remove(&self.connection_id);
        self.metrics.disconnect(self.reason);
        info!(
            connection_id = %self.connection_id,
            reason = ?self.reason,
            "SSE client disconnected"
        );
    }
}

fn random_connection_id() -> String {
    let mut bytes = [0u8; 8];
    rand::rng().fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What the stream loop does next.
enum Step {
    Forward(String),
    Wait,
    End(DisconnectReason),
}

#[allow(
    clippy::cast_possible_wrap,
    reason = "seconds since the Unix epoch; wrapping i64 would require a clock set beyond the year 292 billion"
)]
#[allow(
    clippy::result_large_err,
    reason = "axum handler: the error is the response itself, built once per refused request"
)]
pub async fn sse_handler(
    ClientIp(client_ip): ClientIp,
    State(state): State<AppState>,
    ctx: AuthContext,
    req: axum::http::Request<axum::body::Body>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, Response> {
    let metrics = Arc::clone(&state.metrics);
    if !state.lifecycle.is_ready() {
        metrics.reject(RejectReason::draining);
        return Err(unavailable("This instance is shutting down."));
    }
    let Some(slot) = state.sse_limits.try_reserve() else {
        metrics.reject(RejectReason::capacity);
        warn!(client_ip = %client_ip, "SSE connection refused — instance at SSE_MAX_CONNECTIONS");
        return Err(unavailable("Too many open streams on this instance."));
    };
    let internal = || {
        metrics.reject(RejectReason::bootstrap_error);
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    };

    let sdk_key = req.extensions().get::<SdkKeyEntry>();
    let environment_id = sdk_key.and_then(|key| key.environment_id.clone());
    let sdk_key_name = sdk_key.map(|key| key.name.clone());
    let claims = get_session_claims(&ctx);

    let started = Instant::now();
    let Some(permit) = state.sse_limits.bootstrap_permit().await else {
        metrics.reject(RejectReason::bootstrap_busy);
        warn!(client_ip = %client_ip, "SSE bootstrap queue full — refusing connection");
        return Err(unavailable("Server busy; retry shortly."));
    };
    let env_ids: Vec<String> = if let Some(env_id) = &environment_id {
        vec![env_id.clone()]
    } else if let Some(claims) = &claims {
        sqlx::query_scalar(
            "SELECT e.id::text FROM environments e WHERE $2 = 'admin' OR EXISTS( \
             SELECT 1 FROM project_members pm WHERE pm.project_id = e.project_id AND pm.user_id = $1)",
        ).bind(claims.user_id).bind(&claims.role).fetch_all(&state.db).await
            .map_err(|_| internal())?
    } else {
        // Only validated legacy SDK_KEY credentials have unscoped machine access.
        sqlx::query_scalar("SELECT id::text FROM environments")
            .fetch_all(&state.db)
            .await
            .map_err(|_| internal())?
    };
    let allowed_envs: HashSet<String> = env_ids.iter().cloned().collect();
    // Read bootstrap before opening the stream. A query failure must not signal an empty, ready store.
    // Subscribing first means no event committed after the snapshot is missed.
    let rx = state.flag_tx.subscribe();
    let mut bootstrap = Vec::new();
    let mut flag_count = 0usize;
    // One consistent snapshot: each flag's revision must match the segments it
    // is expanded with, and a tombstone must not race the flags it shadows.
    let mut tx = state.db.begin().await.map_err(|_| internal())?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(|_| internal())?;
    for env_id in &env_ids {
        let segments = crate::api::segments::load_env_segments(env_id, &mut *tx)
            .await
            .map_err(|_| internal())?;
        let rows = sqlx::query(
            "SELECT data, revision FROM flags WHERE environment_id = $1::uuid ORDER BY key",
        )
        .bind(env_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| internal())?;
        for row in rows {
            let data: serde_json::Value = row.try_get("data").map_err(|_| internal())?;
            let revision: i64 = row.try_get("revision").map_err(|_| internal())?;
            let flag = serde_json::from_value(data).map_err(|_| internal())?;
            let flag = crate::api::segments::expand_flag_with_segments(flag, &segments);
            bootstrap.push(crate::api::flags::upsert_message(env_id, &flag, revision));
            flag_count = flag_count.saturating_add(1);
        }
        // Recently deleted flags, so a change event published before the
        // delete but delivered after this bootstrap can't bring one back.
        let tombstones = sqlx::query(
            "SELECT t.key, t.revision FROM flag_tombstones t \
             WHERE t.environment_id = $1::uuid AND NOT EXISTS ( \
                 SELECT 1 FROM flags f WHERE f.environment_id = t.environment_id AND f.key = t.key) \
             ORDER BY t.key",
        )
        .bind(env_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| internal())?;
        for row in tombstones {
            let key: String = row.try_get("key").map_err(|_| internal())?;
            let revision: i64 = row.try_get("revision").map_err(|_| internal())?;
            bootstrap.push(crate::api::flags::delete_message(env_id, &key, revision));
        }
    }
    tx.commit().await.map_err(|_| internal())?;
    drop(permit);
    metrics
        .sse_bootstrap_duration
        .observe(started.elapsed().as_secs_f64());

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
            client_ip,
            connected_at,
        },
    );
    metrics.sse_connections.inc();

    info!(
        client_ip = %client_ip,
        connection_id = %connection_id,
        environment_id = ?environment_id,
        sdk_key_name = ?sdk_key_name,
        bootstrap_flags = flag_count,
        bootstrap_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "SSE client connected"
    );

    // Created here, not inside the stream, so dropping a stream that was
    // never polled still unregisters the client and frees its slot.
    let mut guard = ConnectionGuard {
        clients: Arc::clone(&state.connected_clients),
        connection_id,
        metrics: Arc::clone(&metrics),
        reason: DisconnectReason::client,
        _slot: slot,
    };
    let mut rx = rx;
    let shutdown = state.lifecycle.shutdown.clone();
    let spread = state.sync.config.resync_spread;

    let stream = async_stream::stream! {
        // Announce the resolved environment so SDK clients know where to report
        // impressions (POST /api/environments/{environment_id}/impressions).
        // `null` for session-auth (dashboard) connections, which do not report.
        let connected_payload = serde_json::json!({
            "environment_id": environment_id.clone(),
        })
        .to_string();
        yield Ok(Event::default().event("connected").data(connected_payload));

        for payload in bootstrap {
            yield Ok(Event::default().event("update").data(payload));
        }

        // Signal end of the initial state dump so SDK clients can resolve connect()
        // deterministically (instead of guessing when bootstrap finished). Sent on
        // every (re)connect after the full flag set has been replayed.
        yield Ok(Event::default().event("ready").data(flag_count.to_string()));

        // Stream live deltas, filtering by environment. A resync closes the
        // stream after a random delay within SSE_RESYNC_SPREAD_MS, so the
        // clients of a whole instance do not reconnect in the same instant;
        // newer events keep flowing until then.
        let mut resync_at: Option<tokio::time::Instant> = None;
        loop {
            let step = tokio::select! {
                biased;
                () = shutdown.cancelled() => Step::End(DisconnectReason::shutdown),
                () = async {
                    match resync_at {
                        Some(at) => tokio::time::sleep_until(at).await,
                        None => std::future::pending().await,
                    }
                } => Step::End(DisconnectReason::resync),
                msg = rx.recv() => match msg {
                    Ok(event) => match event.kind {
                        EventKind::Resync => {
                            let ours = event.env_id.as_ref().is_none_or(|e| allowed_envs.contains(e));
                            if ours && resync_at.is_none() {
                                let now = tokio::time::Instant::now();
                                resync_at = Some(now.checked_add(crate::sync::resync_delay(spread)).unwrap_or(now));
                            }
                            Step::Wait
                        }
                        EventKind::Upsert | EventKind::Delete
                            if event.env_id.as_ref().is_some_and(|e| allowed_envs.contains(e)) =>
                        {
                            Step::Forward(event.payload.clone())
                        }
                        _ => Step::Wait,
                    },
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        metrics.sse_lagged_messages.inc_by(n);
                        warn!(
                            client_ip = %client_ip,
                            missed_messages = n,
                            "SSE client lagged — closing connection to trigger reconnect"
                        );
                        Step::End(DisconnectReason::lagged)
                    }
                    Err(broadcast::error::RecvError::Closed) => Step::End(DisconnectReason::shutdown),
                },
            };
            match step {
                Step::Forward(payload) => yield Ok(Event::default().event("update").data(payload)),
                Step::Wait => {}
                Step::End(reason) => {
                    guard.reason = reason;
                    break;
                }
            }
        }
        drop(guard);
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
#[allow(
    clippy::result_large_err,
    reason = "axum handler: the error is the response itself, built once per refused request"
)]
pub async fn flags_snapshot_handler(
    State(state): State<AppState>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<
    (
        axum::http::HeaderMap,
        Json<Vec<checkgate_core::evaluator::Flag>>,
    ),
    Response,
> {
    let env_id = req
        .extensions()
        .get::<SdkKeyEntry>()
        .and_then(|key| key.environment_id.clone())
        .ok_or_else(|| StatusCode::BAD_REQUEST.into_response())?;

    // Polling fallback is what SDKs do when streams fail en masse, so it
    // shares the bootstrap slots rather than adding its own load.
    let started = Instant::now();
    let Some(permit) = state.sse_limits.bootstrap_permit().await else {
        warn!("flags_snapshot: bootstrap queue full — refusing request");
        return Err(unavailable("Server busy; retry shortly."));
    };
    let segment_map = crate::api::segments::load_env_segments(&env_id, &state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;

    let rows =
        sqlx::query("SELECT data FROM flags WHERE environment_id = $1::uuid ORDER BY key ASC")
            .bind(&env_id)
            .fetch_all(&state.db)
            .await
            .map_err(|e| {
                warn!(error = %e, "flags_snapshot: DB query failed");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            })?;
    drop(permit);
    state
        .metrics
        .snapshot_duration
        .observe(started.elapsed().as_secs_f64());

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
        HeaderValue::from_str(&env_id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?,
    );
    Ok((headers, Json(flags)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_defaults_and_validation() {
        let l = SseLimits::from_lookup(10, |_| None).unwrap();
        assert_eq!(l.max_connections, Some(10_000));
        assert_eq!(l.bootstrap.available_permits(), 5);
        assert_eq!(
            SseLimits::from_lookup(1, |_| None)
                .unwrap()
                .bootstrap
                .available_permits(),
            1
        );
        let unlimited =
            SseLimits::from_lookup(10, |n| (n == "SSE_MAX_CONNECTIONS").then(|| "0".into()))
                .unwrap();
        assert_eq!(unlimited.max_connections, None);
        assert!(
            SseLimits::from_lookup(10, |n| (n == "SSE_BOOTSTRAP_CONCURRENCY")
                .then(|| "0".into()))
            .is_err()
        );
    }

    #[test]
    fn slots_are_capped_and_released() {
        let l = Arc::new(
            SseLimits::from_lookup(10, |n| (n == "SSE_MAX_CONNECTIONS").then(|| "2".into()))
                .unwrap(),
        );
        let a = l.try_reserve().unwrap();
        let _b = l.try_reserve().unwrap();
        assert!(l.try_reserve().is_none());
        drop(a);
        assert_eq!(l.active(), 1);
        assert!(l.try_reserve().is_some());
    }
}
