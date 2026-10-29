//! Prometheus/OpenMetrics instrumentation.
//!
//! Every metric is registered once at startup in [`Metrics::new`] and updated
//! with atomic operations, so recording never blocks a request. Labels are
//! fixed, low-cardinality sets: HTTP route *templates* (never raw paths),
//! status classes, and small enums. Flag keys, environment ids, SDK keys,
//! users, addresses and connection ids are never labels.
//!
//! Values that are cheaper to read than to track (pool size, open SSE
//! connections, the sync index size) are sampled when `/metrics` is scraped.
//!
//! The endpoint is served on a separate listener (`METRICS_ADDR`, off by
//! default) so it is never reachable through the public port. Built on
//! `prometheus-client`, whose data model matches OpenMetrics and therefore
//! OpenTelemetry's Prometheus bridge.

use axum::{
    extract::{MatchedPath, Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{Histogram, exponential_buckets};
use prometheus_client::metrics::info::Info;
use prometheus_client::registry::{Registry, Unit};
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct HttpLabels {
    pub method: String,
    pub route: String,
    /// `1xx` … `5xx`.
    pub status_class: &'static str,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct RouteLabels {
    pub method: String,
    pub route: String,
}

/// Why an SSE connection ended.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
#[allow(
    non_camel_case_types,
    reason = "label values are rendered from variant names"
)]
pub enum DisconnectReason {
    /// The client went away (or the response was dropped).
    client,
    /// The client fell more than the broadcast capacity behind.
    lagged,
    /// A resync asked every affected client to re-bootstrap.
    resync,
    /// The server is shutting down.
    shutdown,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct DisconnectLabels {
    pub reason: DisconnectReason,
}

/// Why an SSE connection was refused before streaming.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
#[allow(
    non_camel_case_types,
    reason = "label values are rendered from variant names"
)]
pub enum RejectReason {
    /// `SSE_MAX_CONNECTIONS` reached.
    capacity,
    /// No bootstrap slot freed up in time (`SSE_BOOTSTRAP_CONCURRENCY`).
    bootstrap_busy,
    /// The bootstrap read failed (database error).
    bootstrap_error,
    /// The instance is draining for shutdown.
    draining,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct RejectLabels {
    pub reason: RejectReason,
}

/// What made this instance tell its SSE clients to re-bootstrap.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
#[allow(
    non_camel_case_types,
    reason = "label values are rendered from variant names"
)]
pub enum ResyncTrigger {
    /// This instance's Redis subscription was re-established.
    subscriber_reconnect,
    /// Another instance broadcast a resync after failing to publish.
    remote,
    /// Reconciliation found an environment whose changes were not relayed.
    reconciliation,
    /// Reconciliation found an environment that no longer exists.
    environment_removed,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct ResyncLabels {
    pub trigger: ResyncTrigger,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
#[allow(
    non_camel_case_types,
    reason = "label values are rendered from variant names"
)]
pub enum Outcome {
    success,
    failure,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct OutcomeLabels {
    pub outcome: Outcome,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
#[allow(
    non_camel_case_types,
    reason = "label values are rendered from variant names"
)]
pub enum RedisDisconnect {
    /// The subscription stream ended (connection closed or reset).
    closed,
    /// The connection stayed open but stopped answering pings.
    ping_timeout,
    /// Connecting or subscribing failed.
    connect_failed,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct RedisDisconnectLabels {
    pub reason: RedisDisconnect,
}

type HistogramFamily<L> = Family<L, Histogram, fn() -> Histogram>;

fn latency_buckets() -> Histogram {
    // 1 ms … ~16 s.
    Histogram::new(exponential_buckets(0.001, 2.0, 15))
}

pub struct Metrics {
    registry: Registry,

    pub http_requests: Family<HttpLabels, Counter>,
    pub http_duration: HistogramFamily<RouteLabels>,

    pub flag_updates: Counter,
    pub flag_update_publish_failures: Counter,
    pub flag_updates_received: Counter,
    pub flag_publish_duration: Histogram,

    pub redis_subscriber_connected: Gauge,
    pub redis_reconnections: Family<RedisDisconnectLabels, Counter>,

    pub resync_requests: Family<ResyncLabels, Counter>,
    pub resync_broadcast_failures: Counter,
    pub resync_pending: Gauge,

    pub reconciliations: Family<OutcomeLabels, Counter>,
    pub reconcile_duration: Histogram,
    pub sync_divergences: Counter,
    pub sync_tracked_flags: Gauge,

    pub sse_connections: Counter,
    pub sse_rejections: Family<RejectLabels, Counter>,
    pub sse_disconnects: Family<DisconnectLabels, Counter>,
    pub sse_lagged_messages: Counter,
    pub sse_active: Gauge,
    pub sse_bootstrap_duration: Histogram,
    pub snapshot_duration: Histogram,

    pub db_pool_connections: Gauge,
    pub db_pool_idle_connections: Gauge,
    pub db_pool_max_connections: Gauge,
    pub db_up: Gauge,
    pub ready: Gauge,
}

impl Metrics {
    pub fn new(db_pool_max: u32) -> Arc<Self> {
        let mut r = Registry::default();
        let p = |name: &str| format!("checkgate_{name}");

        let http_requests = Family::<HttpLabels, Counter>::default();
        r.register(
            p("http_requests"),
            "HTTP requests handled, by route template and status class",
            http_requests.clone(),
        );
        let http_duration: HistogramFamily<RouteLabels> =
            Family::new_with_constructor(latency_buckets);
        r.register_with_unit(
            p("http_request_duration"),
            "Time to produce the response head; for /stream that is the bootstrap, not the stream's lifetime",
            Unit::Seconds,
            http_duration.clone(),
        );

        let flag_updates = Counter::default();
        r.register(
            p("flag_updates"),
            "Flag change events this instance published (or tried to)",
            flag_updates.clone(),
        );
        let flag_update_publish_failures = Counter::default();
        r.register(
            p("flag_update_publish_failures"),
            "Change events Redis did not accept after a retry; delivered locally, other instances recover by resync",
            flag_update_publish_failures.clone(),
        );
        let flag_updates_received = Counter::default();
        r.register(
            p("flag_updates_received"),
            "Change events received from the Redis subscription",
            flag_updates_received.clone(),
        );
        let flag_publish_duration = latency_buckets();
        r.register_with_unit(
            p("flag_publish_duration"),
            "Time to publish one change event, retries included",
            Unit::Seconds,
            flag_publish_duration.clone(),
        );

        let redis_subscriber_connected = Gauge::default();
        r.register(
            p("redis_connected"),
            "1 while the Redis subscription is established and answering pings",
            redis_subscriber_connected.clone(),
        );
        let redis_reconnections = Family::<RedisDisconnectLabels, Counter>::default();
        r.register(
            p("redis_reconnections"),
            "Redis subscription losses, by cause",
            redis_reconnections.clone(),
        );

        let resync_requests = Family::<ResyncLabels, Counter>::default();
        r.register(
            p("resync_requests"),
            "Times this instance told SSE clients to re-bootstrap, by trigger",
            resync_requests.clone(),
        );
        let resync_broadcast_failures = Counter::default();
        r.register(
            p("resync_failures"),
            "Failed attempts to broadcast a RESYNC through Redis",
            resync_broadcast_failures.clone(),
        );
        let resync_pending = Gauge::default();
        r.register(
            p("resync_pending"),
            "1 while a RESYNC broadcast is waiting for Redis",
            resync_pending.clone(),
        );

        let reconciliations = Family::<OutcomeLabels, Counter>::default();
        r.register(
            p("sync_reconciliations"),
            "Reconciliation passes against PostgreSQL, by outcome",
            reconciliations.clone(),
        );
        let reconcile_duration = latency_buckets();
        r.register_with_unit(
            p("sync_reconcile_duration"),
            "Duration of one reconciliation pass",
            Unit::Seconds,
            reconcile_duration.clone(),
        );
        let sync_divergences = Counter::default();
        r.register(
            p("sync_divergences"),
            "Environments found with committed changes this instance never relayed (lost notifications)",
            sync_divergences.clone(),
        );
        let sync_tracked_flags = Gauge::default();
        r.register(
            p("sync_tracked_flags"),
            "Flag revisions held by the reconciliation index",
            sync_tracked_flags.clone(),
        );

        let sse_connections = Counter::default();
        r.register(
            p("sse_connections"),
            "SSE streams opened",
            sse_connections.clone(),
        );
        let sse_rejections = Family::<RejectLabels, Counter>::default();
        r.register(
            p("sse_rejections"),
            "SSE streams refused before streaming, by reason",
            sse_rejections.clone(),
        );
        let sse_disconnects = Family::<DisconnectLabels, Counter>::default();
        r.register(
            p("sse_disconnects"),
            "SSE streams ended, by reason",
            sse_disconnects.clone(),
        );
        let sse_lagged_messages = Counter::default();
        r.register(
            p("sse_lagged_messages"),
            "Events skipped by SSE clients that fell behind (each such client is disconnected)",
            sse_lagged_messages.clone(),
        );
        let sse_active = Gauge::default();
        r.register(
            p("sse_active_connections"),
            "Open SSE streams on this instance",
            sse_active.clone(),
        );
        let sse_bootstrap_duration = latency_buckets();
        r.register_with_unit(
            p("sse_bootstrap_duration"),
            "Time to read an SSE bootstrap snapshot, including waiting for a slot",
            Unit::Seconds,
            sse_bootstrap_duration.clone(),
        );
        let snapshot_duration = latency_buckets();
        r.register_with_unit(
            p("snapshot_query_duration"),
            "Time to read a /flags/snapshot response, including waiting for a slot",
            Unit::Seconds,
            snapshot_duration.clone(),
        );

        let db_pool_connections = Gauge::default();
        r.register(
            p("db_pool_connections"),
            "Open PostgreSQL pool connections",
            db_pool_connections.clone(),
        );
        let db_pool_idle_connections = Gauge::default();
        r.register(
            p("db_pool_idle_connections"),
            "Idle PostgreSQL pool connections",
            db_pool_idle_connections.clone(),
        );
        let db_pool_max_connections = Gauge::default();
        db_pool_max_connections.set(i64::from(db_pool_max));
        r.register(
            p("db_pool_max_connections"),
            "Configured PostgreSQL pool size (DB_MAX_CONNECTIONS)",
            db_pool_max_connections.clone(),
        );
        let db_up = Gauge::default();
        r.register(
            p("database_up"),
            "1 if the last PostgreSQL check (readiness probe or reconciliation) succeeded",
            db_up.clone(),
        );
        let ready = Gauge::default();
        r.register(
            p("ready"),
            "1 while the instance reports ready (not draining)",
            ready.clone(),
        );

        r.register(
            p("build"),
            "Build information",
            Info::new(vec![(
                "version".to_string(),
                env!("CARGO_PKG_VERSION").to_string(),
            )]),
        );

        // Every enumerated series exists from the start at zero, so alerts and
        // `rate()` see a 0 → 1 step instead of a series appearing from nothing.
        for reason in [
            RedisDisconnect::closed,
            RedisDisconnect::ping_timeout,
            RedisDisconnect::connect_failed,
        ] {
            drop(redis_reconnections.get_or_create(&RedisDisconnectLabels { reason }));
        }
        for trigger in [
            ResyncTrigger::subscriber_reconnect,
            ResyncTrigger::remote,
            ResyncTrigger::reconciliation,
            ResyncTrigger::environment_removed,
        ] {
            drop(resync_requests.get_or_create(&ResyncLabels { trigger }));
        }
        for reason in [
            RejectReason::capacity,
            RejectReason::bootstrap_busy,
            RejectReason::bootstrap_error,
            RejectReason::draining,
        ] {
            drop(sse_rejections.get_or_create(&RejectLabels { reason }));
        }
        for reason in [
            DisconnectReason::client,
            DisconnectReason::lagged,
            DisconnectReason::resync,
            DisconnectReason::shutdown,
        ] {
            drop(sse_disconnects.get_or_create(&DisconnectLabels { reason }));
        }
        for outcome in [Outcome::success, Outcome::failure] {
            drop(reconciliations.get_or_create(&OutcomeLabels { outcome }));
        }

        Arc::new(Self {
            registry: r,
            http_requests,
            http_duration,
            flag_updates,
            flag_update_publish_failures,
            flag_updates_received,
            flag_publish_duration,
            redis_subscriber_connected,
            redis_reconnections,
            resync_requests,
            resync_broadcast_failures,
            resync_pending,
            reconciliations,
            reconcile_duration,
            sync_divergences,
            sync_tracked_flags,
            sse_connections,
            sse_rejections,
            sse_disconnects,
            sse_lagged_messages,
            sse_active,
            sse_bootstrap_duration,
            snapshot_duration,
            db_pool_connections,
            db_pool_idle_connections,
            db_pool_max_connections,
            db_up,
            ready,
        })
    }

    pub fn resync(&self, trigger: ResyncTrigger) {
        self.resync_requests
            .get_or_create(&ResyncLabels { trigger })
            .inc();
    }

    pub fn disconnect(&self, reason: DisconnectReason) {
        self.sse_disconnects
            .get_or_create(&DisconnectLabels { reason })
            .inc();
    }

    pub fn reject(&self, reason: RejectReason) {
        self.sse_rejections
            .get_or_create(&RejectLabels { reason })
            .inc();
    }

    pub fn redis_lost(&self, reason: RedisDisconnect) {
        self.redis_reconnections
            .get_or_create(&RedisDisconnectLabels { reason })
            .inc();
    }

    pub fn reconciled(&self, outcome: Outcome) {
        self.reconciliations
            .get_or_create(&OutcomeLabels { outcome })
            .inc();
    }

    /// Renders the registry in the OpenMetrics text format.
    pub fn encode(&self) -> Result<String, std::fmt::Error> {
        let mut out = String::new();
        prometheus_client::encoding::text::encode(&mut out, &self.registry)?;
        Ok(out)
    }
}

pub fn status_class(status: StatusCode) -> &'static str {
    match status.as_u16() {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        _ => "5xx",
    }
}

/// The method as a label: the standard verbs, everything else folded into
/// `OTHER` so a client cannot mint label values.
fn method_label(method: &axum::http::Method) -> String {
    use axum::http::Method;
    match *method {
        Method::GET
        | Method::POST
        | Method::PUT
        | Method::PATCH
        | Method::DELETE
        | Method::HEAD
        | Method::OPTIONS => method.as_str().to_string(),
        _ => "OTHER".to_string(),
    }
}

/// Records request count and latency by route template. Requests no route
/// matched (dashboard assets, unknown paths) share the `static` route label.
pub async fn track_http(State(metrics): State<Arc<Metrics>>, req: Request, next: Next) -> Response {
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "static".to_string(), |p| p.as_str().to_string());
    let method = method_label(req.method());
    let start = Instant::now();
    let response = next.run(req).await;
    let elapsed = start.elapsed().as_secs_f64();
    metrics
        .http_duration
        .get_or_create(&RouteLabels {
            method: method.clone(),
            route: route.clone(),
        })
        .observe(elapsed);
    metrics
        .http_requests
        .get_or_create(&HttpLabels {
            method,
            route,
            status_class: status_class(response.status()),
        })
        .inc();
    response
}

/// `GET /metrics` on the metrics listener.
pub async fn metrics_handler(State(state): State<crate::state::AppState>) -> Response {
    let m = &state.metrics;
    let pool_size = i64::from(state.db.size());
    let idle = i64::try_from(state.db.num_idle()).unwrap_or(i64::MAX);
    m.db_pool_connections.set(pool_size);
    m.db_pool_idle_connections.set(idle);
    m.sse_active
        .set(i64::try_from(state.connected_clients.len()).unwrap_or(i64::MAX));
    m.sync_tracked_flags
        .set(i64::try_from(state.sync.index.tracked()).unwrap_or(i64::MAX));
    m.ready.set(i64::from(state.lifecycle.is_ready()));
    match m.encode() {
        Ok(body) => (
            [(
                header::CONTENT_TYPE,
                "application/openmetrics-text; version=1.0.0; charset=utf-8",
            )],
            body,
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
