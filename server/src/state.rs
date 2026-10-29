use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;
use dashmap::DashMap;
use redis::aio::ConnectionManager;
use sqlx::PgPool;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::{RwLock, broadcast};

/// A single SDK key row loaded from the database.
#[derive(Clone, Debug)]
pub struct SdkKeyEntry {
    pub id: i64,
    pub name: String,
    pub value: String,
    /// UUID of the environment this key is scoped to (used for SSE stream filtering).
    /// `None` for legacy env-var keys that carry no environment scope.
    pub environment_id: Option<String>,
}

/// Live SSE connection tracked in-memory for the SDK health dashboard.
#[derive(Clone, Debug)]
pub struct ConnectedClient {
    /// Random UUID assigned when the connection opens.
    pub connection_id: String,
    /// Environment the client is scoped to (None for session-auth / dashboard connections).
    pub environment_id: Option<String>,
    /// Human-readable SDK key name resolved at connect time.
    pub sdk_key_name: Option<String>,
    /// Client address, as resolved by `crate::client_ip`.
    pub client_ip: std::net::IpAddr,
    /// Unix timestamp (seconds) when the connection was established.
    pub connected_at: i64,
}

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub redis_client: redis::Client,
    /// Shared Redis connection for pub/sub publishes. A `ConnectionManager`
    /// reconnects by itself after the connection breaks.
    pub redis_conn: ConnectionManager,
    /// Set while a publish has failed and a `RESYNC` broadcast is still being
    /// retried (see `flags::publish_update`).
    pub resync_pending: Arc<AtomicBool>,
    /// Broadcast channel: the Redis subscriber pushes payloads here;
    /// each SSE handler subscribes instead of opening its own Redis connection.
    pub flag_tx: broadcast::Sender<String>,
    /// Local key-management cache. Authentication always checks PostgreSQL.
    pub sdk_keys: Arc<RwLock<Vec<SdkKeyEntry>>>,
    /// Encryption key for `PrivateCookieJar`. Derived from `SESSION_SECRET` env var.
    pub session_key: Key,
    /// Live SSE connections keyed by connection_id. Per-instance only: each
    /// server reports its own clients on the SDK health page.
    pub connected_clients: Arc<DashMap<String, ConnectedClient>>,
    /// Shared HTTP client for outbound webhook delivery.
    pub webhook_client: reqwest::Client,
    /// Operator-configured `SETUP_TOKEN`; `None` means per-instance generated
    /// tokens, verified by hash against `settings` (see `crate::setup`).
    pub setup_token: Option<Arc<str>>,
}

/// Allows `PrivateCookieJar` extractors to pull the key out of `AppState` automatically.
impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.session_key.clone()
    }
}
