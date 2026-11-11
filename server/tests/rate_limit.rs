#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! Client identity and per-class rate limits, through the real server binary.
//! The tests connect from 127.0.0.1, so whether that address is listed in
//! `TRUSTED_PROXIES` decides whether `X-Forwarded-For` is believed.

mod common;

use common::*;
use futures_util::StreamExt as _;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A failed login. Each uses a new email so the per-account lockout (5
/// failures) never answers before the rate limiter does.
async fn login_attempt(server: &Server, xff: Option<&str>) -> reqwest::Response {
    static ATTEMPT: AtomicUsize = AtomicUsize::new(0);
    let n = ATTEMPT.fetch_add(1, Ordering::Relaxed);
    let mut req = reqwest::Client::new()
        .post(server.url("/api/auth/login"))
        .header("X-Checkgate-Request", "true")
        .json(&json!({ "email": format!("nobody-{n}@acme.test"), "password": "wrong-password" }));
    if let Some(xff) = xff {
        req = req.header("X-Forwarded-For", xff);
    }
    req.send().await.unwrap()
}

async fn lockout_ips(db_url: &str) -> Vec<String> {
    let pool = sqlx::PgPool::connect(db_url).await.unwrap();
    let ips = sqlx::query_scalar("SELECT DISTINCT ip FROM login_attempts ORDER BY ip")
        .fetch_all(&pool)
        .await
        .unwrap();
    pool.close().await;
    ips
}

#[tokio::test]
async fn spoofed_forwarded_for_from_a_direct_client_is_ignored() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_rl_direct").await;
    let server = Server::spawn(
        &db,
        &env.redis_url,
        &[("RATE_LIMIT_AUTH_PER_MINUTE", Some("3"))],
    )
    .await;

    // Rotating the header does not buy a fresh budget.
    for i in 0..3 {
        let r = login_attempt(&server, Some(&format!("198.51.100.{i}"))).await;
        assert_eq!(r.status(), 401, "attempt {i} reaches the handler");
    }
    let r = login_attempt(&server, Some("198.51.100.200")).await;
    assert_eq!(r.status(), 429);
    let retry_after: u64 = r.headers()["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(retry_after >= 1);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["retry_after_seconds"], retry_after);
    assert!(body["error"].is_string(), "dashboard reads `error`");

    // Nor does it change who the attempts are recorded against.
    assert_eq!(lockout_ips(&db).await, vec!["127.0.0.1".to_string()]);

    // Other budgets are independent of the exhausted one...
    let r = reqwest::Client::new()
        .get(server.url("/api/auth/workspace"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "API budget unaffected");
    // ...and health checks are never limited.
    for _ in 0..100 {
        let r = reqwest::get(server.url("/health")).await.unwrap();
        assert_eq!(r.status(), 200);
    }
}

#[tokio::test]
async fn trusted_proxy_forwards_distinct_client_identities() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_rl_trusted").await;
    let server = Server::spawn(
        &db,
        &env.redis_url,
        &[
            ("RATE_LIMIT_AUTH_PER_MINUTE", Some("2")),
            ("TRUSTED_PROXIES", Some("127.0.0.1/32")),
        ],
    )
    .await;

    for client in ["198.51.100.1", "198.51.100.2"] {
        for _ in 0..2 {
            assert_eq!(login_attempt(&server, Some(client)).await.status(), 401);
        }
        assert_eq!(
            login_attempt(&server, Some(client)).await.status(),
            429,
            "{client} has its own budget"
        );
    }
    // Multi-hop: the client-supplied left part is ignored; the right-most
    // untrusted hop is the client.
    assert_eq!(
        login_attempt(&server, Some("198.51.100.1, 203.0.113.9, 127.0.0.1"))
            .await
            .status(),
        401,
        "203.0.113.9 is a new client"
    );
    // Garbage in the header falls back to the proxy's own budget.
    assert_eq!(
        login_attempt(&server, Some("not-an-ip")).await.status(),
        401
    );
    assert_eq!(login_attempt(&server, None).await.status(), 401);
    assert_eq!(login_attempt(&server, Some("")).await.status(), 429);

    assert_eq!(
        lockout_ips(&db).await,
        vec!["127.0.0.1", "198.51.100.1", "198.51.100.2", "203.0.113.9"],
        "lockout records the resolved client"
    );
}

#[tokio::test]
async fn untrusted_proxy_cannot_forward_identities() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_rl_untrusted").await;
    // 127.0.0.1 is not in the trusted range, so it is treated as the client.
    let server = Server::spawn(
        &db,
        &env.redis_url,
        &[
            ("RATE_LIMIT_AUTH_PER_MINUTE", Some("2")),
            ("TRUSTED_PROXIES", Some("10.0.0.0/8")),
        ],
    )
    .await;
    assert_eq!(
        login_attempt(&server, Some("198.51.100.1")).await.status(),
        401
    );
    assert_eq!(
        login_attempt(&server, Some("198.51.100.2")).await.status(),
        401
    );
    assert_eq!(
        login_attempt(&server, Some("198.51.100.3")).await.status(),
        429
    );
}

#[tokio::test]
async fn invalid_rate_limit_or_proxy_configuration_is_refused() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_rl_config").await;
    let log = expect_startup_failure(
        &db,
        &env.redis_url,
        &[("TRUSTED_PROXIES", Some("10.0.0.0/8, proxy.internal"))],
    )
    .await;
    assert!(log.contains("TRUSTED_PROXIES"), "{log}");
    let log = expect_startup_failure(
        &db,
        &env.redis_url,
        &[("RATE_LIMIT_SSE_PER_MINUTE", Some("plenty"))],
    )
    .await;
    assert!(log.contains("RATE_LIMIT_SSE_PER_MINUTE"), "{log}");
}

/// Reads SSE frames until one with `event: update` arrives for `flag_key`.
async fn next_update_for<B: AsRef<[u8]>>(
    stream: &mut (impl futures_util::Stream<Item = reqwest::Result<B>> + Unpin),
    buf: &mut String,
    flag_key: &str,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        while let Some(end) = buf.find("\n\n") {
            let frame: String = buf.drain(..end + 2).collect();
            if frame.contains("event: update") && frame.contains(&format!("\"key\":\"{flag_key}\""))
            {
                return;
            }
        }
        let chunk = tokio::time::timeout_at(deadline, stream.next())
            .await
            .expect("update arrives in time")
            .expect("stream open")
            .unwrap();
        buf.push_str(&String::from_utf8_lossy(chunk.as_ref()));
    }
}

#[tokio::test]
async fn sse_limits_connections_not_delivered_events() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_rl_sse").await;
    let server = Server::spawn(
        &db,
        &env.redis_url,
        &[("RATE_LIMIT_SSE_PER_MINUTE", Some("2"))],
    )
    .await;
    let (admin, sdk_key, env_id) = complete_setup(&server).await;
    let sdk = bearer_client(&sdk_key);

    let first = sdk.get(server.url("/stream")).send().await.unwrap();
    assert_eq!(first.status(), 200);
    let mut events = first.bytes_stream();
    let mut buf = String::new();

    // Far more events than the connection budget flow over the open stream.
    let flags = server.url(&format!("/api/environments/{env_id}/flags"));
    for i in 0..10 {
        let key = format!("sse-flag-{i}");
        request(
            &admin,
            "POST",
            &flags,
            Some(json!({ "key": key, "is_enabled": true, "flag_type": "boolean", "rules": [] })),
            200,
        )
        .await;
        next_update_for(&mut events, &mut buf, &key).await;
    }

    // Opening streams is what is counted.
    assert_eq!(
        sdk.get(server.url("/stream"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        sdk.get(server.url("/stream"))
            .send()
            .await
            .unwrap()
            .status(),
        429,
        "third connection in a minute"
    );
    // Snapshot polling has its own budget.
    assert_eq!(
        sdk.get(server.url("/flags/snapshot"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
}
