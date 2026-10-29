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
mod notify;
mod rate_limit;
mod scheduler;
mod setup;
mod state;
mod stream;
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
use tokio::sync::{RwLock, broadcast};
use tokio_stream::StreamExt;
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

    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://checkgate:password@localhost/checkgate".to_string());

    let max_conns = std::env::var("DB_MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(10);

    info!(max_connections = max_conns, "Connecting to PostgreSQL");

    let db = PgPoolOptions::new()
        .max_connections(max_conns)
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

    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());

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
        info!("TRUSTED_PROXIES set — X-Forwarded-For is honoured from those peers only");
    }
    let trusted_proxies = Arc::new(trusted_proxies);

    let rate_limit_config = RateLimitConfig::from_env().map_err(|e| {
        error!(error = %e, "Invalid rate limit configuration");
        e
    })?;
    info!(config = ?rate_limit_config, "Rate limits per client per minute");
    let rate_limiters = Arc::new(RateLimiters::new(rate_limit_config));
    rate_limiters.spawn_eviction();

    // ── Broadcast channel ─────────────────────────────────────────────────────

    const BROADCAST_CHANNEL_CAPACITY: usize = 256;
    let (flag_tx, _) = broadcast::channel::<String>(BROADCAST_CHANNEL_CAPACITY);

    // ── Redis pub/sub subscriber ───────────────────────────────────────────────

    {
        let redis_sub = redis_client.clone();
        let tx = flag_tx.clone();

        tokio::spawn(async move {
            let mut first_subscription = true;
            loop {
                let mut con = match redis_sub.get_async_pubsub().await {
                    Ok(c) => c,
                    Err(e) => {
                        error!(error = %e, "Redis subscriber: connection failed — retrying in 2s");
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        continue;
                    }
                };

                if let Err(e) = con.subscribe("checkgate_updates").await {
                    error!(error = %e, "Redis subscriber: subscribe failed — retrying in 2s");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }

                info!("Redis subscriber: listening on channel 'checkgate_updates'");

                // Anything published while the subscription was down never
                // reached this instance, so make every local SSE client
                // reconnect and re-bootstrap from PostgreSQL.
                if !first_subscription {
                    warn!("Redis subscriber: resubscribed — resyncing local SSE clients");
                    let _ = tx.send(api::flags::RESYNC_MESSAGE.to_string());
                }
                first_subscription = false;

                let mut msg_stream = con.into_on_message();

                while let Some(msg) = msg_stream.next().await {
                    let payload = match msg.get_payload::<String>() {
                        Ok(p) => p,
                        Err(e) => {
                            warn!(error = %e, "Redis subscriber: failed to decode message payload");
                            continue;
                        }
                    };

                    // Payloads are forwarded as-is: each SSE handler parses them
                    // and delivers only events for the environments it serves.
                    // There is no server-side flag cache to update — SSE
                    // bootstrap, snapshots and the REST API all read PostgreSQL.
                    if tx.receiver_count() > 0
                        && let Err(e) = tx.send(payload)
                    {
                        warn!(error = %e, "Broadcast send failed — no active SSE receivers");
                    }
                }

                warn!("Redis subscriber: connection dropped — reconnecting");
            }
        });
    }

    // ── App state ─────────────────────────────────────────────────────────────

    let webhook_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("Checkgate-Webhooks/1.0")
        .build()
        .unwrap_or_default();

    let app_state = state::AppState {
        db: db.clone(),
        redis_client,
        redis_conn,
        resync_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        flag_tx,
        sdk_keys,
        session_key,
        connected_clients: Arc::new(DashMap::new()),
        webhook_client,
        setup_token,
    };

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
    {
        let sched_state = app_state.clone();
        tokio::spawn(async move { scheduler::run(sched_state).await });
    }

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

    let app = Router::new()
        .route("/health", get(|| async { "OK" }))
        .nest("/api/auth", auth_routes)
        .nest("/api", setup_routes)
        .merge(protected)
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
        .layer(cors)
        .fallback_service(serve_dashboard)
        .with_state(app_state);

    // ── Listen ────────────────────────────────────────────────────────────────

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let bind_addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .map_err(|e| {
            error!(error = %e, bind_addr, "Failed to bind TCP listener");
            e
        })?;

    info!(addr = %listener.local_addr()?, "Server listening");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    info!("Server shut down gracefully");
    Ok(())
}

#[allow(
    clippy::expect_used,
    reason = "process startup: a kernel that will not install a signal handler is a fatal, non-recoverable condition and there is no request in flight to fail"
)]
async fn shutdown_signal() {
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
        _ = ctrl_c => { info!("Received Ctrl+C — initiating graceful shutdown"); },
        _ = terminate => { info!("Received SIGTERM — initiating graceful shutdown"); },
    }
}
