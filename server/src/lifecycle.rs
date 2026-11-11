//! Liveness, readiness, and graceful shutdown.
//!
//! - `GET /health/live`: the process is serving requests. Never consults a
//!   dependency, so a PostgreSQL or Redis outage cannot get healthy pods
//!   restarted in a loop.
//! - `GET /health/ready`: whether this instance should receive traffic.
//!   PostgreSQL is required (every read and write goes to it). Redis is not
//!   by default: without it, writes still commit and are delivered locally,
//!   and other instances recover through reconciliation, so the instance is
//!   reported `degraded` but kept in service. `READY_REQUIRES_REDIS=true`
//!   makes a lost Redis subscription fail readiness instead. A draining
//!   instance is never ready.
//! - `GET /health`: unchanged plain `OK`, kept for existing probes.
//!
//! Shutdown (SIGTERM/SIGINT): readiness fails at once; after
//! `SHUTDOWN_DRAIN_DELAY_SECS` (time for load balancers to notice) every SSE
//! stream is closed and background tasks are cancelled; in-flight requests
//! get until `SHUTDOWN_GRACE_PERIOD_SECS` after the signal, then the process
//! exits regardless.

use crate::state::AppState;
use axum::{Json, extract::State, http::StatusCode};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::info;

const DEPENDENCY_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleConfig {
    pub drain_delay: Duration,
    pub grace_period: Duration,
    pub ready_requires_redis: bool,
}

impl LifecycleConfig {
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let secs = |name: &str, default: u64| -> Result<Duration, String> {
            match lookup(name) {
                None => Ok(Duration::from_secs(default)),
                Some(v) => v
                    .trim()
                    .parse::<u64>()
                    .map(Duration::from_secs)
                    .map_err(|_| format!("{name} must be a non-negative integer, got {v:?}")),
            }
        };
        let drain_delay = secs("SHUTDOWN_DRAIN_DELAY_SECS", 0)?;
        let grace_period = secs("SHUTDOWN_GRACE_PERIOD_SECS", 20)?;
        if drain_delay > grace_period {
            return Err(
                "SHUTDOWN_DRAIN_DELAY_SECS must not exceed SHUTDOWN_GRACE_PERIOD_SECS".into(),
            );
        }
        let ready_requires_redis = match lookup("READY_REQUIRES_REDIS").as_deref().map(str::trim) {
            None | Some("" | "false" | "0") => false,
            Some("true" | "1") => true,
            Some(other) => {
                return Err(format!(
                    "READY_REQUIRES_REDIS must be true or false, got {other:?}"
                ));
            }
        };
        Ok(Self {
            drain_delay,
            grace_period,
            ready_requires_redis,
        })
    }
}

pub struct Lifecycle {
    pub config: LifecycleConfig,
    draining: AtomicBool,
    /// Fired when SSE streams and background tasks must stop.
    pub shutdown: CancellationToken,
}

impl Lifecycle {
    pub fn new(config: LifecycleConfig) -> Self {
        Self {
            config,
            draining: AtomicBool::new(false),
            shutdown: CancellationToken::new(),
        }
    }

    pub fn is_ready(&self) -> bool {
        !self.draining.load(Ordering::Acquire)
    }

    pub fn begin_drain(&self) {
        self.draining.store(true, Ordering::Release);
    }
}

pub async fn live() -> Json<Value> {
    Json(json!({ "status": "alive" }))
}

pub async fn ready(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    if !state.lifecycle.is_ready() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "draining" })),
        );
    }
    let database_ok = matches!(
        tokio::time::timeout(
            DEPENDENCY_CHECK_TIMEOUT,
            sqlx::query("SELECT 1").execute(&state.db)
        )
        .await,
        Ok(Ok(_))
    );
    state.metrics.db_up.set(i64::from(database_ok));
    let redis_ok = state.metrics.redis_subscriber_connected.get() == 1;
    let ready = database_ok && (redis_ok || !state.lifecycle.config.ready_requires_redis);
    let status = match (ready, redis_ok) {
        (false, _) => "unavailable",
        (true, true) => "ready",
        (true, false) => "degraded",
    };
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({
            "status": status,
            "checks": {
                "database": if database_ok { "ok" } else { "unavailable" },
                "redis": if redis_ok { "ok" } else { "unavailable" },
            }
        })),
    )
}

#[allow(
    clippy::expect_used,
    reason = "process startup: a kernel that will not install a signal handler is a fatal, non-recoverable condition and there is no request in flight to fail"
)]
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => info!(signal = "SIGINT", "Shutdown requested"),
        () = terminate => info!(signal = "SIGTERM", "Shutdown requested"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_and_validation() {
        let c = LifecycleConfig::from_lookup(|_| None).unwrap();
        assert_eq!(c.drain_delay, Duration::ZERO);
        assert_eq!(c.grace_period, Duration::from_secs(20));
        assert!(!c.ready_requires_redis);
        let set = |pairs: &'static [(&'static str, &'static str)]| {
            LifecycleConfig::from_lookup(move |n| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == n)
                    .map(|(_, v)| v.to_string())
            })
        };
        assert!(
            set(&[("READY_REQUIRES_REDIS", "true")])
                .unwrap()
                .ready_requires_redis
        );
        assert!(set(&[("READY_REQUIRES_REDIS", "maybe")]).is_err());
        assert!(set(&[("SHUTDOWN_DRAIN_DELAY_SECS", "30")]).is_err());
        assert!(set(&[("SHUTDOWN_GRACE_PERIOD_SECS", "-1")]).is_err());
    }
}
