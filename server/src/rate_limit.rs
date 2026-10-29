//! Per-client rate limiting with a separate budget for each kind of endpoint.
//!
//! Clients are identified by [`ClientIp`], which only honours forwarding
//! headers from trusted proxies (see [`crate::client_ip`]). Each route class
//! has its own quota, so a burst of SDK polling cannot lock an operator out of
//! the dashboard and a password-guessing run cannot spend the SDK budget.
//!
//! The middleware runs once per HTTP request. An SSE stream is one request, so
//! only opening it is counted; events delivered on an open stream never are.

use crate::client_ip::ClientIp;
use axum::{
    Json,
    extract::State,
    http::{HeaderValue, Method, Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter, clock::Clock};
use std::{
    net::{IpAddr, Ipv6Addr},
    num::NonZeroU32,
    sync::Arc,
    time::Duration,
};

/// How often idle per-client entries are dropped from the limiter maps.
const EVICTION_INTERVAL: Duration = Duration::from_secs(60);

/// The budget a request is charged to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteClass {
    /// `/health`: probes from orchestrators and load balancers are never limited.
    Exempt,
    /// Password login and first-run setup — the credential-guessing surface.
    Auth,
    /// Dashboard and automation REST API.
    Api,
    /// SDK snapshot polling (`/flags/snapshot`).
    Sdk,
    /// Opening an SSE stream (`/stream`).
    SseConnect,
    /// Impression and event ingestion from SDKs.
    Ingest,
}

impl RouteClass {
    fn name(self) -> &'static str {
        match self {
            RouteClass::Exempt => "exempt",
            RouteClass::Auth => "auth",
            RouteClass::Api => "api",
            RouteClass::Sdk => "sdk",
            RouteClass::SseConnect => "sse_connect",
            RouteClass::Ingest => "ingest",
        }
    }
}

/// Maps a request to its budget.
pub fn classify(method: &Method, path: &str) -> RouteClass {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match (method, segments.as_slice()) {
        (_, ["health"]) => RouteClass::Exempt,
        (&Method::POST, ["api", "auth", "login"]) | (_, ["api", "setup", ..]) => RouteClass::Auth,
        (_, ["stream"]) => RouteClass::SseConnect,
        (_, ["flags", "snapshot"]) => RouteClass::Sdk,
        (&Method::POST, ["api", "environments", _, "impressions" | "events"]) => RouteClass::Ingest,
        _ => RouteClass::Api,
    }
}

/// Requests per minute allowed per client for each class. `None` disables
/// limiting for that class (for deployments that limit at a gateway).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimitConfig {
    pub auth: Option<NonZeroU32>,
    pub api: Option<NonZeroU32>,
    pub sdk: Option<NonZeroU32>,
    pub sse_connect: Option<NonZeroU32>,
    pub ingest: Option<NonZeroU32>,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            auth: NonZeroU32::new(20),
            api: NonZeroU32::new(300),
            sdk: NonZeroU32::new(600),
            sse_connect: NonZeroU32::new(120),
            ingest: NonZeroU32::new(600),
        }
    }
}

impl RateLimitConfig {
    /// Reads `RATE_LIMIT_{AUTH,API,SDK,SSE,INGEST}_PER_MINUTE`. Unset keeps the
    /// default, `0` disables the class, anything else must be a positive integer.
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let defaults = Self::default();
        let read = |name: &str, default: Option<NonZeroU32>| match lookup(name) {
            None => Ok(default),
            Some(v) => v
                .trim()
                .parse::<u32>()
                .map(NonZeroU32::new)
                .map_err(|_| format!("{name} must be a non-negative integer, got {v:?}")),
        };
        Ok(Self {
            auth: read("RATE_LIMIT_AUTH_PER_MINUTE", defaults.auth)?,
            api: read("RATE_LIMIT_API_PER_MINUTE", defaults.api)?,
            sdk: read("RATE_LIMIT_SDK_PER_MINUTE", defaults.sdk)?,
            sse_connect: read("RATE_LIMIT_SSE_PER_MINUTE", defaults.sse_connect)?,
            ingest: read("RATE_LIMIT_INGEST_PER_MINUTE", defaults.ingest)?,
        })
    }
}

type Limiter = DefaultKeyedRateLimiter<IpAddr>;

/// One keyed limiter per route class.
pub struct RateLimiters {
    auth: Option<Limiter>,
    api: Option<Limiter>,
    sdk: Option<Limiter>,
    sse_connect: Option<Limiter>,
    ingest: Option<Limiter>,
}

impl RateLimiters {
    pub fn new(config: RateLimitConfig) -> Self {
        // A per-minute quota allows a burst of the whole minute's budget, then
        // refills one request every 60/n seconds.
        let per_minute = |n: Option<NonZeroU32>| n.map(Quota::per_minute);
        Self::with_quotas(
            per_minute(config.auth),
            per_minute(config.api),
            per_minute(config.sdk),
            per_minute(config.sse_connect),
            per_minute(config.ingest),
        )
    }

    fn with_quotas(
        auth: Option<Quota>,
        api: Option<Quota>,
        sdk: Option<Quota>,
        sse_connect: Option<Quota>,
        ingest: Option<Quota>,
    ) -> Self {
        let build = |q: Option<Quota>| q.map(RateLimiter::keyed);
        Self {
            auth: build(auth),
            api: build(api),
            sdk: build(sdk),
            sse_connect: build(sse_connect),
            ingest: build(ingest),
        }
    }

    fn limiter(&self, class: RouteClass) -> Option<&Limiter> {
        match class {
            RouteClass::Exempt => None,
            RouteClass::Auth => self.auth.as_ref(),
            RouteClass::Api => self.api.as_ref(),
            RouteClass::Sdk => self.sdk.as_ref(),
            RouteClass::SseConnect => self.sse_connect.as_ref(),
            RouteClass::Ingest => self.ingest.as_ref(),
        }
    }

    fn all(&self) -> impl Iterator<Item = &Limiter> {
        [
            &self.auth,
            &self.api,
            &self.sdk,
            &self.sse_connect,
            &self.ingest,
        ]
        .into_iter()
        .flatten()
    }

    /// Charges one request to `class` for `ip`. On refusal, returns how long
    /// until the next request would be accepted.
    pub fn check(&self, class: RouteClass, ip: IpAddr) -> Result<(), Duration> {
        let Some(limiter) = self.limiter(class) else {
            return Ok(());
        };
        limiter
            .check_key(&rate_key(ip))
            .map_err(|not_until| not_until.wait_time_from(limiter.clock().now()))
    }

    /// Drops entries whose bucket has fully refilled — they are
    /// indistinguishable from a client never seen — so the maps hold only
    /// clients active within the last refill period.
    pub fn evict_idle(&self) {
        for limiter in self.all() {
            limiter.retain_recent();
            limiter.shrink_to_fit();
        }
    }

    /// Number of tracked client keys across all classes.
    pub fn tracked_keys(&self) -> usize {
        self.all().map(Limiter::len).sum()
    }

    /// Runs [`Self::evict_idle`] in the background for the life of the process.
    pub fn spawn_eviction(self: &Arc<Self>) {
        let limiters = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(EVICTION_INTERVAL);
            loop {
                tick.tick().await;
                limiters.evict_idle();
                tracing::debug!(
                    tracked_keys = limiters.tracked_keys(),
                    "Rate limiter idle entries evicted"
                );
            }
        });
    }
}

/// The key a client is limited under. An IPv6 client usually controls a whole
/// /64, so keying single addresses would hand it 2^64 fresh budgets (and grow
/// the map without bound); the /64 prefix is the unit it actually owns.
fn rate_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => {
            let mut segments = v6.segments();
            for host in segments.iter_mut().skip(4) {
                *host = 0;
            }
            IpAddr::V6(Ipv6Addr::from(segments))
        }
    }
}

fn too_many_requests(wait: Duration) -> Response {
    let retry_after = wait.as_secs().saturating_add(1);
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(serde_json::json!({
            "error": "Too many requests. Please slow down and try again shortly.",
            "retry_after_seconds": retry_after,
        })),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(retry_after));
    response
}

pub async fn rate_limit(
    State(limiters): State<Arc<RateLimiters>>,
    ClientIp(client_ip): ClientIp,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let class = classify(req.method(), req.uri().path());
    match limiters.check(class, client_ip) {
        Ok(()) => next.run(req).await,
        Err(wait) => {
            tracing::warn!(
                client_ip = %client_ip,
                class = class.name(),
                path = %req.uri().path(),
                "Rate limit exceeded"
            );
            too_many_requests(wait)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_ip::{TrustedProxies, resolve_client_ip};
    use axum::{
        Router,
        body::Body,
        extract::ConnectInfo,
        middleware,
        routing::{get, post},
    };
    use std::net::SocketAddr;
    use tower::ServiceExt;

    fn nz(n: u32) -> Option<NonZeroU32> {
        NonZeroU32::new(n)
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn classification() {
        let cases = [
            (Method::GET, "/health", RouteClass::Exempt),
            (Method::POST, "/api/auth/login", RouteClass::Auth),
            (Method::GET, "/api/auth/me", RouteClass::Api),
            (Method::POST, "/api/auth/logout", RouteClass::Api),
            (Method::GET, "/api/setup/key", RouteClass::Auth),
            (Method::POST, "/api/setup/complete", RouteClass::Auth),
            (Method::GET, "/stream", RouteClass::SseConnect),
            (Method::GET, "/flags/snapshot", RouteClass::Sdk),
            (
                Method::POST,
                "/api/environments/e/impressions",
                RouteClass::Ingest,
            ),
            (
                Method::POST,
                "/api/environments/e/events",
                RouteClass::Ingest,
            ),
            (Method::GET, "/api/environments/e/events", RouteClass::Api),
            (Method::GET, "/api/environments/e/flags", RouteClass::Api),
            (Method::POST, "/api/projects", RouteClass::Api),
            (Method::GET, "/api/health/connections", RouteClass::Api),
        ];
        for (method, path, want) in cases {
            assert_eq!(classify(&method, path), want, "{method} {path}");
        }
    }

    #[test]
    fn config_from_env_values() {
        let config = RateLimitConfig::from_lookup(|name| match name {
            "RATE_LIMIT_AUTH_PER_MINUTE" => Some("5".into()),
            "RATE_LIMIT_INGEST_PER_MINUTE" => Some("0".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(config.auth, nz(5));
        assert_eq!(config.ingest, None, "0 disables the class");
        assert_eq!(config.api, RateLimitConfig::default().api);
        assert!(
            RateLimitConfig::from_lookup(
                |n| (n == "RATE_LIMIT_API_PER_MINUTE").then(|| "lots".to_string())
            )
            .is_err()
        );
        assert!(
            RateLimitConfig::from_lookup(
                |n| (n == "RATE_LIMIT_SDK_PER_MINUTE").then(|| "-1".to_string())
            )
            .is_err()
        );
    }

    #[test]
    fn budgets_are_independent_per_class_and_client() {
        let limiters = RateLimiters::new(RateLimitConfig {
            auth: nz(2),
            api: nz(3),
            sdk: nz(1),
            sse_connect: nz(1),
            ingest: None,
        });
        let a = ip("198.51.100.1");
        let b = ip("198.51.100.2");
        assert!(limiters.check(RouteClass::Auth, a).is_ok());
        assert!(limiters.check(RouteClass::Auth, a).is_ok());
        assert!(
            limiters.check(RouteClass::Auth, a).is_err(),
            "auth exhausted"
        );
        // Other classes and other clients are untouched.
        for _ in 0..3 {
            assert!(limiters.check(RouteClass::Api, a).is_ok());
        }
        assert!(limiters.check(RouteClass::Api, a).is_err());
        assert!(limiters.check(RouteClass::Sdk, a).is_ok());
        assert!(limiters.check(RouteClass::SseConnect, a).is_ok());
        assert!(limiters.check(RouteClass::Auth, b).is_ok());
        // Disabled and exempt classes never refuse.
        for _ in 0..1000 {
            assert!(limiters.check(RouteClass::Ingest, a).is_ok());
            assert!(limiters.check(RouteClass::Exempt, a).is_ok());
        }
    }

    #[test]
    fn ipv6_clients_share_their_slash_64() {
        let limiters = RateLimiters::new(RateLimitConfig {
            auth: nz(1),
            ..RateLimitConfig::default()
        });
        assert!(
            limiters
                .check(RouteClass::Auth, ip("2001:db8:1:2::1"))
                .is_ok()
        );
        assert!(
            limiters
                .check(RouteClass::Auth, ip("2001:db8:1:2:ffff::9"))
                .is_err(),
            "same /64"
        );
        assert!(
            limiters
                .check(RouteClass::Auth, ip("2001:db8:1:3::1"))
                .is_ok()
        );
        // IPv4-mapped IPv6 is the IPv4 client.
        assert!(limiters.check(RouteClass::Auth, ip("192.0.2.1")).is_ok());
        assert!(
            limiters
                .check(RouteClass::Auth, ip("::ffff:192.0.2.1"))
                .is_err()
        );
    }

    #[test]
    fn idle_entries_are_evicted() {
        let fast = Quota::with_period(Duration::from_millis(5)).unwrap();
        let limiters = RateLimiters::with_quotas(Some(fast), None, None, None, None);
        for i in 0..50u8 {
            limiters
                .check(RouteClass::Auth, IpAddr::from([198, 51, 100, i]))
                .unwrap();
        }
        assert_eq!(limiters.tracked_keys(), 50);
        std::thread::sleep(Duration::from_millis(30));
        limiters.evict_idle();
        assert_eq!(limiters.tracked_keys(), 0, "refilled buckets are dropped");
    }

    // --- Middleware, driven through a real router ---------------------------

    fn app(trusted: &str, config: RateLimitConfig) -> Router {
        let limiters = Arc::new(RateLimiters::new(config));
        let trusted = Arc::new(TrustedProxies::parse(trusted).unwrap());
        Router::new()
            .route("/health", get(|| async { "OK" }))
            .route("/api/auth/login", post(|| async { "login" }))
            .route("/api/projects", get(|| async { "projects" }))
            .route("/stream", get(|| async { "stream" }))
            .route(
                "/whoami",
                get(|ClientIp(ip): ClientIp| async move { ip.to_string() }),
            )
            .layer(middleware::from_fn_with_state(limiters, rate_limit))
            .layer(middleware::from_fn_with_state(trusted, resolve_client_ip))
    }

    async fn send(
        app: &Router,
        method: Method,
        path: &str,
        peer: &str,
        xff: Option<&str>,
    ) -> Response {
        let mut req = Request::builder().method(method).uri(path);
        if let Some(xff) = xff {
            req = req.header("x-forwarded-for", xff);
        }
        let mut req = req.body(Body::empty()).unwrap();
        let peer: SocketAddr = format!("{peer}:40000").parse().unwrap();
        req.extensions_mut().insert(ConnectInfo(peer));
        app.clone().oneshot(req).await.unwrap()
    }

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 16)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn tight() -> RateLimitConfig {
        RateLimitConfig {
            auth: nz(2),
            api: nz(3),
            sdk: nz(3),
            sse_connect: nz(1),
            ingest: nz(3),
        }
    }

    #[tokio::test]
    async fn spoofed_header_cannot_escape_the_budget() {
        let app = app("", tight());
        // A direct client rotating X-Forwarded-For still spends one budget.
        for i in 0..2 {
            let r = send(
                &app,
                Method::POST,
                "/api/auth/login",
                "203.0.113.5",
                Some(&format!("198.51.100.{i}")),
            )
            .await;
            assert_eq!(r.status(), StatusCode::OK);
        }
        let r = send(
            &app,
            Method::POST,
            "/api/auth/login",
            "203.0.113.5",
            Some("198.51.100.99"),
        )
        .await;
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        // And the header does not change who the request is attributed to.
        let r = send(
            &app,
            Method::GET,
            "/whoami",
            "203.0.113.5",
            Some("198.51.100.1"),
        )
        .await;
        assert_eq!(body_text(r).await, "203.0.113.5");
    }

    #[tokio::test]
    async fn trusted_proxy_clients_get_separate_budgets() {
        let app = app("10.0.0.0/8", tight());
        for client in ["198.51.100.1", "198.51.100.2"] {
            for _ in 0..2 {
                let r = send(
                    &app,
                    Method::POST,
                    "/api/auth/login",
                    "10.0.0.7",
                    Some(client),
                )
                .await;
                assert_eq!(r.status(), StatusCode::OK, "{client}");
            }
            let r = send(
                &app,
                Method::POST,
                "/api/auth/login",
                "10.0.0.7",
                Some(client),
            )
            .await;
            assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS, "{client}");
        }
        let r = send(
            &app,
            Method::GET,
            "/whoami",
            "10.0.0.7",
            Some("1.1.1.1, 198.51.100.3, 10.0.0.9"),
        )
        .await;
        assert_eq!(body_text(r).await, "198.51.100.3", "multi-hop chain");
    }

    #[tokio::test]
    async fn untrusted_proxy_is_limited_as_itself() {
        let app = app("10.0.0.0/8", tight());
        for client in ["198.51.100.1", "198.51.100.2"] {
            let r = send(
                &app,
                Method::POST,
                "/api/auth/login",
                "192.0.2.50",
                Some(client),
            )
            .await;
            assert_eq!(r.status(), StatusCode::OK);
        }
        let r = send(
            &app,
            Method::POST,
            "/api/auth/login",
            "192.0.2.50",
            Some("198.51.100.3"),
        )
        .await;
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn invalid_forwarded_header_falls_back_to_the_proxy() {
        let app = app("10.0.0.0/8", tight());
        for bad in ["garbage", "", "198.51.100.1, nope"] {
            let r = send(&app, Method::GET, "/whoami", "10.0.0.7", Some(bad)).await;
            assert_eq!(body_text(r).await, "10.0.0.7", "{bad:?}");
        }
    }

    #[tokio::test]
    async fn refusal_is_a_429_with_retry_after_and_json_body() {
        let app = app("", tight());
        for _ in 0..3 {
            send(&app, Method::GET, "/api/projects", "203.0.113.5", None).await;
        }
        let r = send(&app, Method::GET, "/api/projects", "203.0.113.5", None).await;
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry: u64 = r.headers()[header::RETRY_AFTER]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!((1..=61).contains(&retry), "retry-after {retry}");
        let body: serde_json::Value = serde_json::from_str(&body_text(r).await).unwrap();
        assert_eq!(body["retry_after_seconds"], retry);
        assert!(body["error"].is_string());
    }

    #[tokio::test]
    async fn classes_do_not_share_budgets_through_the_middleware() {
        let app = app("", tight());
        for _ in 0..3 {
            send(&app, Method::GET, "/api/projects", "203.0.113.5", None).await;
        }
        assert_eq!(
            send(&app, Method::GET, "/api/projects", "203.0.113.5", None)
                .await
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            send(&app, Method::POST, "/api/auth/login", "203.0.113.5", None)
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            send(&app, Method::GET, "/stream", "203.0.113.5", None)
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            send(&app, Method::GET, "/stream", "203.0.113.5", None)
                .await
                .status(),
            StatusCode::TOO_MANY_REQUESTS,
            "one SSE connection per minute in this config"
        );
    }

    #[tokio::test]
    async fn health_checks_are_never_limited() {
        let app = app("", tight());
        for _ in 0..3 {
            send(&app, Method::GET, "/api/projects", "203.0.113.5", None).await;
        }
        for _ in 0..500 {
            assert_eq!(
                send(&app, Method::GET, "/health", "203.0.113.5", None)
                    .await
                    .status(),
                StatusCode::OK
            );
        }
    }
}
