use axum::{
    extract::{ConnectInfo, State},
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use std::{net::IpAddr, num::NonZeroU32, sync::Arc};

pub type IpRateLimiter = Arc<DefaultKeyedRateLimiter<IpAddr>>;

/// 60 requests per minute per source IP on all API routes.
///
/// `RATE_PER_MINUTE` is a `const`, so the `NonZeroU32` conversion is checked at
/// compile time — there is no runtime unwrap to fail.
const RATE_PER_MINUTE: NonZeroU32 = match NonZeroU32::new(60) {
    Some(n) => n,
    None => unreachable!(),
};

pub fn new_rate_limiter() -> IpRateLimiter {
    Arc::new(RateLimiter::keyed(Quota::per_minute(RATE_PER_MINUTE)))
}

pub async fn rate_limit(
    State(state): State<crate::state::AppState>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // Prefer the first address in X-Forwarded-For so that rate limiting works
    // correctly behind a reverse proxy. Fall back to the direct TCP peer.
    let client_ip: IpAddr = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(|| addr.ip());

    match state.rate_limiter.check_key(&client_ip) {
        Ok(_) => Ok(next.run(req).await),
        Err(_) => {
            tracing::warn!(
                client_ip = %client_ip,
                path = %req.uri().path(),
                "Rate limit exceeded"
            );
            Err(StatusCode::TOO_MANY_REQUESTS)
        }
    }
}
