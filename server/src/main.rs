#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )
)]

mod api;
mod auth;
mod client_ip;
mod integrations;
mod lifecycle;
mod metrics;
mod notify;
mod rate_limit;
mod scheduler;
mod setup;
mod state;
mod stream;
mod sync;
mod webhook_fire;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{Method, header},
    middleware,
    routing::get,
};
use axum_extra::extract::cookie::Key;
use dashmap::DashMap;
use rand::RngExt as _;
use rate_limit::{RateLimitConfig, RateLimiters};
use sqlx::{Row, postgres::PgPoolOptions};
use state::SdkKeyEntry;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, RwLock, broadcast};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::{DefaultMakeSpan, DefaultOnFailure, DefaultOnResponse, TraceLayer};
use tracing::{Level, error, info, warn};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_current_span(true)
                .with_span_list(false)
                .with_target(true),
        )
        .init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "Starting Checkgate Control Plane"
    );

    // ── PostgreSQL ────────────────────────────────────────────────────────────

    let db_url = connection_url(
        "DATABASE_URL",
        "postgres://checkgate:password@localhost/checkgate",
    )?;

    let max_conns = std::env::var("DB_MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(10);

    // Read every setting before touching a dependency, so a typo stops the
    // process at once with a message naming it.
    let sync_config = sync::SyncConfig::from_env().map_err(|e| {
        error!(error = %e, "Invalid synchronization setting");
        e
    })?;
    let lifecycle_config = lifecycle::LifecycleConfig::from_env().map_err(|e| {
        error!(error = %e, "Invalid shutdown/readiness setting");
        e
    })?;
    let sse_limits = Arc::new(stream::SseLimits::from_env(max_conns).map_err(|e| {
        error!(error = %e, "Invalid SSE limit");
        e
    })?);
    // A peer that stops reading leaves the SSE stream unpolled (its socket is
    // full), so no application timer can fire for it; the kernel can. With
    // TCP_USER_TIMEOUT, a connection whose sent data stays unacknowledged —
    // including behind a zero receive window — this long is aborted.
    let tcp_user_timeout = match std::env::var("TCP_USER_TIMEOUT_SECS") {
        Ok(v) if !v.trim().is_empty() => v.trim().parse::<u64>().map_err(|_| {
            error!(value = %v, "TCP_USER_TIMEOUT_SECS must be a non-negative integer");
            format!("invalid TCP_USER_TIMEOUT_SECS {v:?}")
        })?,
        _ => 60,
    };
    let tcp_user_timeout = (tcp_user_timeout > 0).then(|| Duration::from_secs(tcp_user_timeout));

    let metrics_addr = match std::env::var("METRICS_ADDR") {
        Ok(v) if !v.trim().is_empty() => {
            Some(v.trim().parse::<std::net::SocketAddr>().map_err(|_| {
                error!(value = %v, "METRICS_ADDR must be host:port, e.g. 0.0.0.0:9464");
                format!("invalid METRICS_ADDR {v:?}")
            })?)
        }
        _ => None,
    };
    info!(
        reconcile_interval = ?sync_config.reconcile_interval,
        reconcile_grace = ?sync_config.reconcile_grace,
        redis_channel = sync::updates_channel(),
        "Synchronization settings"
    );

    info!(max_connections = max_conns, "Connecting to PostgreSQL");

    let db = PgPoolOptions::new()
        .max_connections(max_conns)
        // Fail a request that cannot get a connection, rather than holding it
        // for sqlx's default 30 s while the pool is exhausted.
        .acquire_timeout(Duration::from_secs(10))
        .connect(&db_url)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to connect to PostgreSQL");
            e
        })?;

    info!("PostgreSQL connection established");

    sqlx::migrate!().run(&db).await.map_err(|e| {
        error!(error = %e, "Failed to run database migrations");
        e
    })?;

    info!("Migrations complete — schema ready");

    // ── First-run setup token ─────────────────────────────────────────────────

    let setup_token = setup::configured_token().map_err(|e| {
        error!(error = %e, "Invalid SETUP_TOKEN");
        e
    })?;
    setup::prepare(&db, setup_token.as_deref())
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to prepare first-run setup");
            e
        })?;

    // ── Redis ─────────────────────────────────────────────────────────────────

    let redis_url = connection_url("REDIS_URL", "redis://localhost:6379")?;

    info!("Connecting to Redis");

    let redis_client = redis::Client::open(redis_url).map_err(|e| {
        error!(error = %e, "Invalid Redis URL");
        e
    })?;

    // Shared connection used for pub/sub publishes from handlers. A
    // ConnectionManager is Clone (handlers clone it cheaply) and, unlike a bare
    // MultiplexedConnection, replaces itself after the connection breaks.
    // Short timeouts keep a Redis outage from stalling write requests.
    let redis_conn = redis_client
        .get_connection_manager_with_config(
            redis::aio::ConnectionManagerConfig::new()
                .set_connection_timeout(Some(Duration::from_secs(2)))
                .set_response_timeout(Some(Duration::from_secs(2)))
                .set_number_of_retries(3)
                .set_max_delay(Duration::from_secs(2)),
        )
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to connect to Redis");
            e
        })?;

    info!("Redis connection established");

    // ── SDK keys (DB-backed) ──────────────────────────────────────────────────

    let rows =
        sqlx::query("SELECT id, name, value, environment_id::text FROM sdk_keys ORDER BY id ASC")
            .fetch_all(&db)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to load SDK keys");
                e
            })?;

    let mut sdk_keys_data: Vec<SdkKeyEntry> = rows
        .iter()
        .map(|r| SdkKeyEntry {
            id: r.get("id"),
            name: r.get("name"),
            value: r.get("value"),
            environment_id: Some(r.get("environment_id")),
        })
        .collect();

    if sdk_keys_data.is_empty() {
        let mut bytes = [0u8; 16];
        rand::rng().fill(&mut bytes);
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let key_value = format!("sk_live_{hex}");

        let row = sqlx::query(
            "INSERT INTO sdk_keys (name, value, environment_id) \
             VALUES ('Default', $1, (SELECT id FROM environments WHERE is_default = true LIMIT 1)) \
             RETURNING id, name, value, environment_id::text",
        )
        .bind(&key_value)
        .fetch_one(&db)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to insert initial SDK key");
            e
        })?;

        info!("Generated initial SDK key — open the dashboard to complete setup");
        sdk_keys_data.push(SdkKeyEntry {
            id: row.get("id"),
            name: row.get("name"),
            value: row.get("value"),
            environment_id: Some(row.get("environment_id")),
        });
    } else {
        info!(count = sdk_keys_data.len(), "SDK keys loaded from database");
    }

    // Legacy: SDK_KEY env var accepted as an extra in-memory key.
    if let Ok(env_key) = std::env::var("SDK_KEY")
        && !env_key.is_empty()
        && !sdk_keys_data.iter().any(|e| e.value == env_key)
    {
        warn!(
            "SDK_KEY env var detected — accepting it as a valid key. Consider migrating to DB-managed keys via the Settings page."
        );
        sdk_keys_data.push(SdkKeyEntry {
            id: -1,
            name: "env:SDK_KEY".into(),
            value: env_key,
            environment_id: None, // env var keys have no environment scope
        });
    }

    let sdk_keys = Arc::new(RwLock::new(sdk_keys_data));

    // ── Session key ───────────────────────────────────────────────────────────

    const DEV_SESSION_KEY: &str = "INSECURE_DEFAULT_DEV_KEY_CHANGE_IN_PRODUCTION_CHECKGATE";

    let session_key = match std::env::var("SESSION_SECRET") {
        Ok(ref s) if s.len() >= 32 => {
            info!("SESSION_SECRET configured — session cookies are cryptographically secure");
            Key::derive_from(s.as_bytes())
        }
        Ok(ref s) if !s.is_empty() => {
            warn!(
                "SESSION_SECRET is shorter than 32 chars; consider a longer secret for production"
            );
            Key::derive_from(s.as_bytes())
        }
        _ => {
            if cfg!(not(debug_assertions)) {
                error!(
                    "SESSION_SECRET is not set. \
                     Refusing to start with the insecure dev key in a release build. \
                     Set SESSION_SECRET to a cryptographically random string of ≥32 characters."
                );
                std::process::exit(1);
            }
            warn!(
                "SESSION_SECRET not set — using insecure dev key; \
                 set SESSION_SECRET to a random ≥32-char string in production"
            );
            Key::derive_from(DEV_SESSION_KEY.as_bytes())
        }
    };

    // ── Client identity & rate limits ─────────────────────────────────────────

    let trusted_proxies = client_ip::TrustedProxies::from_env().map_err(|e| {
        error!(error = %e, "Invalid TRUSTED_PROXIES");
        e
    })?;
    if trusted_proxies.is_empty() {
        info!("TRUSTED_PROXIES not set — clients are identified by their TCP peer address");
    } else {
        info!(
            header = ?trusted_proxies.header(),
            "TRUSTED_PROXIES set — the forwarding header is honoured from those peers only"
        );
    }
    let trusted_proxies = Arc::new(trusted_proxies);

    let rate_limit_config = RateLimitConfig::from_env().map_err(|e| {
        error!(error = %e, "Invalid rate limit configuration");
        e
    })?;
    info!(config = ?rate_limit_config, "Rate limits per client per minute");
    let rate_limiters = Arc::new(RateLimiters::new(rate_limit_config));

    // ── Broadcast channel ─────────────────────────────────────────────────────

    // Per-instance fan-out to SSE connections. A connection that falls this
    // many events behind is closed and reconnects (see `stream.rs`).
    const BROADCAST_CHANNEL_CAPACITY: usize = 256;
    let (flag_tx, _) = broadcast::channel::<Arc<sync::SyncEvent>>(BROADCAST_CHANNEL_CAPACITY);

    // ── App state ─────────────────────────────────────────────────────────────

    let webhook_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("Checkgate-Webhooks/1.0")
        .build()
        .unwrap_or_default();

    let metrics = metrics::Metrics::new(max_conns);
    let lifecycle = Arc::new(lifecycle::Lifecycle::new(lifecycle_config));
    let app_state = state::AppState {
        db: db.clone(),
        redis_client: redis_client.clone(),
        redis_conn,
        resync_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        flag_tx,
        sdk_keys,
        session_key,
        connected_clients: Arc::new(DashMap::new()),
        webhook_client,
        setup_token,
        sync: sync::SyncState::new(sync_config),
        metrics: Arc::clone(&metrics),
        lifecycle: Arc::clone(&lifecycle),
        sse_limits,
    };
    let shutdown = lifecycle.shutdown.clone();
    rate_limiters.spawn_eviction(shutdown.clone());

    // ── Redis subscriber and reconciliation ──────────────────────────────────
    //
    // Order matters for the reconciliation index: subscribe first, then seed
    // the index from PostgreSQL, then accept connections. A change committed
    // after the seed either arrives on the subscription or is caught by the
    // next reconciliation pass.

    let subscribed = Arc::new(Notify::new());
    let subscriber = tokio::spawn(sync::run_subscriber(
        app_state.clone(),
        redis_client,
        shutdown.clone(),
        Arc::clone(&subscribed),
    ));
    if tokio::time::timeout(Duration::from_secs(5), subscribed.notified())
        .await
        .is_err()
    {
        warn!("Redis subscription not established yet — continuing; clients resync once it is");
    }
    if app_state.sync.config.reconcile_interval.is_some()
        && let Err(e) = sync::reconcile_once(&app_state).await
    {
        warn!(error = %e, "Initial sync index seed failed — the reconciler will retry");
    }
    let reconciler = tokio::spawn(sync::run_reconciler(app_state.clone(), shutdown.clone()));

    // ── Middleware stack ───────────────────────────────────────────────────────

    // CORS: restrict allowed request headers to what SDK clients actually need.
    // By NOT listing X-Checkgate-Request here, cross-origin requests cannot include
    // it (the browser will block the preflight), making the CSRF header check
    // effective against third-party origins.
    let cors = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        .allow_origin(tower_http::cors::Any)
        .expose_headers([header::HeaderName::from_static(
            "x-checkgate-environment-id",
        )]);

    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
        .on_response(DefaultOnResponse::new().level(Level::INFO))
        .on_failure(DefaultOnFailure::new().level(Level::ERROR));

    let public_dir = std::env::var("PUBLIC_DIR").unwrap_or_else(|_| "public".to_string());
    let serve_dashboard =
        ServeDir::new(&public_dir).fallback(ServeFile::new(format!("{public_dir}/index.html")));

    // ── Routing ───────────────────────────────────────────────────────────────
    //
    // Public: /api/auth/* and /api/setup/* — no auth middleware.
    // Protected read: any valid auth (session or SDK key).
    // Protected write: additionally requires admin role.
    //
    // Layer execution order for a write request (outermost → innermost):
    //   cors → client_ip → rate_limit → csrf → require_auth → require_editor/require_admin → handler

    let auth_routes = api::auth_router();
    let setup_routes = api::setup_router();

    // Spawn scheduled-change executor (checks every 60 s, multi-instance safe).
    let scheduler = tokio::spawn(scheduler::run(app_state.clone(), shutdown.clone()));

    // Flag write routes: editors and admins can create/edit/delete flags.
    let editor_write_api = api::editor_write_router()
        .merge(api::segment_write_router())
        .merge(api::scheduled_write_router())
        .merge(api::experiment_write_router())
        .layer(middleware::from_fn_with_state(
            app_state.clone(),
            auth::require_editor,
        ));

    // Admin-only write routes: environments, SDK keys, users, webhooks.
    let admin_write_api = api::admin_write_router()
        .merge(api::webhook_write_router())
        .layer(middleware::from_fn_with_state(
            app_state.clone(),
            auth::require_admin,
        ));

    const API_BODY_LIMIT: usize = 65_536;
    const INGEST_BODY_LIMIT: usize = 256 * 1024;

    // Standard API routes (65 KB body limit).
    let api_routes = api::read_router()
        .merge(api::self_service_router())
        .merge(editor_write_api)
        .merge(admin_write_api)
        .layer(DefaultBodyLimit::max(API_BODY_LIMIT));

    // Impression ingest — kept separate so its 256 KB body limit is not overridden
    // by the smaller limit applied to api_routes above.
    let ingest_routes = api::ingest_router().layer(DefaultBodyLimit::max(INGEST_BODY_LIMIT));

    let protected = Router::new()
        .nest("/api", api_routes)
        .nest("/api", ingest_routes)
        .route("/stream", get(stream::sse_handler))
        .route("/flags/snapshot", get(stream::flags_snapshot_handler))
        .layer(middleware::from_fn_with_state(
            app_state.clone(),
            auth::require_auth,
        ));

    // The dashboard fallback is added before the layers: `Router::layer` only
    // wraps what the router already has, and static files need the security
    // headers too.
    let app = Router::new()
        .route("/health", get(|| async { "OK" }))
        .route("/health/live", get(lifecycle::live))
        .route("/health/ready", get(lifecycle::ready))
        .nest("/api/auth", auth_routes)
        .nest("/api", setup_routes)
        .merge(protected)
        .fallback_service(serve_dashboard)
        .layer(trace_layer)
        .layer(middleware::from_fn(api::security_headers))
        .layer(middleware::from_fn(api::csrf_protection))
        .layer(middleware::from_fn_with_state(
            rate_limiters,
            rate_limit::rate_limit,
        ))
        .layer(middleware::from_fn_with_state(
            trusted_proxies,
            client_ip::resolve_client_ip,
        ))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&metrics),
            metrics::track_http,
        ))
        .layer(cors)
        .with_state(app_state.clone());

    // ── Listen ────────────────────────────────────────────────────────────────

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let bind_addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .map_err(|e| {
            error!(error = %e, bind_addr, "Failed to bind TCP listener");
            e
        })?;

    info!(addr = %listener.local_addr()?, tcp_user_timeout = ?tcp_user_timeout, "Server listening");
    let listener = {
        use axum::serve::ListenerExt as _;
        listener.tap_io(move |tcp| {
            #[cfg(any(target_os = "linux", target_os = "android", target_os = "fuchsia"))]
            if let Some(timeout) = tcp_user_timeout
                && let Err(e) = socket2::SockRef::from(&*tcp).set_tcp_user_timeout(Some(timeout))
            {
                tracing::debug!(error = %e, "Could not set TCP_USER_TIMEOUT");
            }
            #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "fuchsia")))]
            let _ = (tcp, tcp_user_timeout);
        })
    };

    // ── Metrics listener ─────────────────────────────────────────────────────
    //
    // Separate from the public port so `/metrics` is only reachable where the
    // operator binds it (a private interface, or a port that is not
    // published). Off unless METRICS_ADDR is set.
    let metrics_server = match metrics_addr {
        Some(addr) => {
            let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
                error!(error = %e, %addr, "Failed to bind metrics listener");
                e
            })?;
            info!(addr = %listener.local_addr()?, "Metrics listening on /metrics");
            let router = Router::new()
                .route("/metrics", get(metrics::metrics_handler))
                .with_state(app_state.clone());
            let token = shutdown.clone();
            Some(tokio::spawn(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(token.cancelled_owned())
                    .await
            }))
        }
        None => {
            info!("METRICS_ADDR not set — /metrics is disabled");
            None
        }
    };

    metrics.ready.set(1);
    let mut server = tokio::spawn(
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown.clone().cancelled_owned())
        .into_future(),
    );

    // ── Shutdown ─────────────────────────────────────────────────────────────

    tokio::select! {
        () = lifecycle::shutdown_signal() => {}
        result = &mut server => {
            error!("HTTP server stopped unexpectedly");
            result??;
            return Err("HTTP server stopped".into());
        }
    }
    let config = lifecycle.config.clone();
    let signalled = tokio::time::Instant::now();
    lifecycle.begin_drain();
    metrics.ready.set(0);
    info!(
        drain_delay = ?config.drain_delay,
        grace_period = ?config.grace_period,
        open_streams = app_state.connected_clients.len(),
        "Draining: readiness now fails; streams close after the drain delay"
    );
    tokio::time::sleep(config.drain_delay).await;
    // Stops accepting connections, ends every SSE stream, and stops the
    // subscriber, reconciler, scheduler, resync retries and limiter eviction.
    shutdown.cancel();

    let deadline = signalled
        .checked_add(config.grace_period)
        .unwrap_or(signalled);
    match tokio::time::timeout_at(deadline, &mut server).await {
        Ok(Ok(Ok(()))) => info!("HTTP server drained"),
        Ok(Ok(Err(e))) => error!(error = %e, "HTTP server failed while draining"),
        Ok(Err(e)) => error!(error = %e, "HTTP server task failed"),
        Err(_) => {
            warn!(
                open_streams = app_state.connected_clients.len(),
                "Grace period elapsed with requests still in flight — exiting anyway"
            );
            server.abort();
        }
    }
    for (name, task) in [
        ("subscriber", subscriber),
        ("reconciler", reconciler),
        ("scheduler", scheduler),
    ] {
        // Each was cancelled above and stops at its next await point.
        if tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .is_err()
        {
            warn!(task = name, "Background task did not stop in time");
        }
    }
    if let Some(task) = metrics_server {
        task.abort();
    }
    if tokio::time::timeout(Duration::from_secs(5), db.close())
        .await
        .is_err()
    {
        warn!("PostgreSQL pool did not close within 5s");
    }

    info!(
        elapsed_ms = u64::try_from(signalled.elapsed().as_millis()).unwrap_or(u64::MAX),
        "Server shut down gracefully"
    );
    Ok(())
}

/// Reads a required connection string. Release builds refuse to start without
/// it rather than quietly connecting to a local default — in a container that
/// could be a different database than the operator intended. Debug builds keep
/// the localhost default for `cargo run`.
fn connection_url(name: &str, dev_default: &str) -> Result<String, String> {
    match std::env::var(name) {
        Ok(url) if !url.trim().is_empty() => Ok(url),
        _ if cfg!(debug_assertions) => {
            warn!(
                variable = name,
                "Not set — using the local development default"
            );
            Ok(dev_default.to_string())
        }
        _ => {
            error!(variable = name, "Required connection setting is not set");
            Err(format!("{name} is not set"))
        }
    }
}
