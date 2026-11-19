#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! Regression tests for the low-severity findings of the v0.1.31 security
//! review that were remediated before release, against real servers:
//!
//! - N1: webhook and integration URLs may not target link-local addresses
//!   (cloud metadata services) or the unspecified address;
//! - N2: flag reads with an SDK key (public by design) no longer carry the
//!   flag owner's email address;
//! - N3: revoking an SDK key closes the streams already open with it, on
//!   every instance;
//! - N4: concurrent failed logins cannot exceed the lockout limit.

mod common;

use common::cluster::Cluster;
use common::*;
use serde_json::{Value, json};
use std::time::Duration;

// ── N1 ───────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn webhooks_and_integrations_refuse_link_local_destinations() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_sec_n1").await;
    let server = Server::spawn(&db, &env.redis_url, &[]).await;
    let (admin, _, env_id) = complete_setup(&server).await;
    let hooks = server.url(&format!("/api/environments/{env_id}/webhooks"));
    let chats = server.url(&format!("/api/environments/{env_id}/integrations"));

    let blocked = [
        "http://169.254.169.254/latest/meta-data/",
        "https://169.254.169.254/computeMetadata/v1/",
        "http://169.254.170.2/v2/credentials",
        "https://[fe80::1]/hook",
        "https://[fd00:ec2::254]/latest",
        "https://[::ffff:169.254.169.254]/x",
        "https://0.0.0.0/hook",
    ];
    let mut accepted = Vec::new();
    for url in blocked {
        let r = admin
            .post(&hooks)
            .header("X-Checkgate-Request", "true")
            .json(&json!({ "name": "meta", "url": url }))
            .send()
            .await
            .unwrap();
        if r.status() != 422 {
            accepted.push(format!("webhook {url}: {}", r.status()));
        }
        if url.starts_with("https://") {
            let r = admin
                .post(&chats)
                .header("X-Checkgate-Request", "true")
                .json(&json!({ "kind": "slack", "name": "meta", "webhook_url": url }))
                .send()
                .await
                .unwrap();
            if r.status() != 422 {
                accepted.push(format!("integration {url}: {}", r.status()));
            }
        }
    }

    // Ordinary destinations, including the loopback and private addresses
    // that self-hosted operators use for internal receivers, still work.
    for url in [
        "https://hooks.example.com/h",
        "http://127.0.0.1:9/h",
        "http://10.0.0.5/h",
    ] {
        request(
            &admin,
            "POST",
            &hooks,
            Some(json!({ "name": "ok", "url": url })),
            200,
        )
        .await;
    }
    let hook = request(
        &admin,
        "POST",
        &hooks,
        Some(json!({ "name": "patched", "url": "https://hooks.example.com/p" })),
        200,
    )
    .await;
    let r = admin
        .patch(format!("{hooks}/{}", hook["id"].as_str().unwrap()))
        .header("X-Checkgate-Request", "true")
        .json(&json!({ "url": "http://169.254.169.254/" }))
        .send()
        .await
        .unwrap();
    if r.status() != 422 {
        accepted.push(format!("webhook patch: {}", r.status()));
    }
    eprintln!("N1 accepted link-local destinations: {accepted:#?}");
    assert!(accepted.is_empty(), "{accepted:#?}");
}

// ── N2 ───────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn sdk_key_flag_reads_omit_the_owner_email() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_sec_n2").await;
    let server = Server::spawn(&db, &env.redis_url, &[]).await;
    let (admin, sdk_key, env_id) = complete_setup(&server).await;
    let flags = server.url(&format!("/api/environments/{env_id}/flags"));
    request(
        &admin,
        "POST",
        &flags,
        Some(json!({
            "key": "owned",
            "is_enabled": true,
            "tags": ["checkout"],
            "owner_email": "owner@acme.test",
        })),
        200,
    )
    .await;

    let sdk = bearer_client(&sdk_key);
    let list: Value = sdk.get(&flags).send().await.unwrap().json().await.unwrap();
    let one: Value = sdk
        .get(format!("{flags}/owned"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let admin_one = request(&admin, "GET", &format!("{flags}/owned"), None, 200).await;
    let admin_list = request(&admin, "GET", &flags, None, 200).await;
    eprintln!("N2 SDK-key list: {list}");
    eprintln!("N2 SDK-key get:  {one}");

    for (label, flag) in [("list", &list[0]), ("get", &one)] {
        assert_eq!(flag["key"], "owned", "{label}");
        assert!(
            flag.get("owner_email").is_none(),
            "{label}: an SDK key must not read the owner's email: {flag}"
        );
        assert_eq!(flag["tags"], json!(["checkout"]), "{label}: tags are kept");
    }
    // Users still see it.
    assert_eq!(admin_one["owner_email"], "owner@acme.test");
    assert_eq!(admin_list[0]["owner_email"], "owner@acme.test");
}

// ── N3 ───────────────────────────────────────────────────────────────────────

/// Opens `GET /stream` with `key` and returns a task that resolves when the
/// server ends the stream (or the 30 s cap passes: `false`).
async fn open_stream(base: &str, key: &str) -> tokio::task::JoinHandle<bool> {
    let mut response = bearer_client(key)
        .get(format!("{base}/stream"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "stream with a valid key");
    // Read up to the bootstrap's `ready` event so the stream is established.
    let mut seen = String::new();
    while !seen.contains("event: ready") {
        let chunk = tokio::time::timeout(Duration::from_secs(10), response.chunk())
            .await
            .expect("bootstrap within 10 s")
            .unwrap()
            .expect("stream open");
        seen.push_str(&String::from_utf8_lossy(&chunk));
    }
    tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(30), async {
            while let Ok(Some(_)) = response.chunk().await {}
        })
        .await
        .is_ok()
    })
}

#[tokio::test]
async fn revoking_a_key_closes_its_open_streams_on_every_instance() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(&env, "cg_sec_n3", 2, &[]).await;
    let projects = request(
        &c.admin,
        "GET",
        &c.server(0).url("/api/projects"),
        None,
        200,
    )
    .await;
    let project = projects[0]["id"].as_str().unwrap().to_string();
    let keys = c.server(0).url(&format!("/api/projects/{project}/keys"));
    let doomed = request(
        &c.admin,
        "POST",
        &keys,
        Some(json!({ "name": "doomed", "environment_id": c.env_id })),
        200,
    )
    .await;
    let doomed_key = doomed["key"].as_str().unwrap().to_string();

    let on_a = open_stream(&c.base(0), &doomed_key).await;
    let on_b = open_stream(&c.base(1), &doomed_key).await;
    let survivor = open_stream(&c.base(1), &c.sdk_key).await;

    let revoked_at = std::time::Instant::now();
    request(
        &c.admin,
        "DELETE",
        &format!("{keys}/{}", doomed["id"]),
        None,
        204,
    )
    .await;

    // Both streams of the revoked key end; the cluster's resync spread is
    // 200 ms, the cap here is 10 s.
    let wait = |h: tokio::task::JoinHandle<bool>| async move {
        tokio::time::timeout(Duration::from_secs(10), h)
            .await
            .map(|r| r.unwrap())
            .unwrap_or(false)
    };
    let (closed_a, closed_b) = (wait(on_a).await, wait(on_b).await);
    let elapsed = revoked_at.elapsed();
    let reconnect = bearer_client(&doomed_key)
        .get(format!("{}/stream", c.base(1)))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16();
    // Streams of other keys in the environment are resynced too (the
    // documented cost); they reconnect and bootstrap as usual.
    let survivor_closed = wait(survivor).await;
    let again = open_stream(&c.base(1), &c.sdk_key).await;
    again.abort();

    eprintln!(
        "N3 after revocation: closed on A {closed_a}, on B {closed_b} ({elapsed:?}); \
         reconnect {reconnect}; other key's stream resynced {survivor_closed}"
    );
    assert!(
        closed_a,
        "the revoked key's stream on the instance that revoked it"
    );
    assert!(closed_b, "the revoked key's stream on another instance");
    assert_eq!(reconnect, 401);
}

// ── N4 ───────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn concurrent_failed_logins_cannot_exceed_the_lockout_limit() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_sec_n4").await;
    // Per-IP auth rate limiting off: every request here comes from 127.0.0.1,
    // and the per-email lockout is what is under test.
    let server = Server::spawn(
        &db,
        &env.redis_url,
        &[("RATE_LIMIT_AUTH_PER_MINUTE", Some("0"))],
    )
    .await;
    complete_setup(&server).await;
    let login = server.url("/api/auth/login");
    let pool = sqlx::PgPool::connect(&db).await.unwrap();
    let recorded = |email: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM login_attempts WHERE email = $1")
                .bind(email)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };

    // Sequential behaviour is unchanged: 4 failures leave 1 attempt, and a
    // correct password then succeeds and clears the history.
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("X-Checkgate-Request", "true".parse().unwrap());
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap();
    for expected_remaining in [4, 3, 2, 1] {
        let r = client
            .post(&login)
            .json(&json!({ "email": ADMIN_EMAIL, "password": "wrong-password" }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401);
        let body: Value = r.json().await.unwrap();
        assert_eq!(body["attempts_remaining"], expected_remaining);
    }
    let r = client
        .post(&login)
        .json(&json!({ "email": ADMIN_EMAIL, "password": ADMIN_PASSWORD }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "correct password before the limit");
    assert_eq!(recorded(ADMIN_EMAIL).await, 0, "success clears the history");

    // 20 concurrent wrong passwords for one account. A checked password logs
    // "Login failed: invalid credentials"; a refused attempt does not.
    let checked = || {
        server
            .log()
            .matches("Login failed: invalid credentials")
            .count()
    };
    let checked_before = checked();
    let attempts: Vec<_> = (0..20)
        .map(|_| {
            let client = client.clone();
            let login = login.clone();
            tokio::spawn(async move {
                let r = client
                    .post(&login)
                    .json(&json!({ "email": ADMIN_EMAIL, "password": "wrong-password" }))
                    .send()
                    .await
                    .unwrap();
                r.status().as_u16()
            })
        })
        .collect();
    let mut statuses = Vec::new();
    for a in attempts {
        statuses.push(a.await.unwrap());
    }
    statuses.sort_unstable();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let passwords_checked = checked() - checked_before;
    let failures_recorded = recorded(ADMIN_EMAIL).await;
    let locked = client
        .post(&login)
        .json(&json!({ "email": ADMIN_EMAIL, "password": ADMIN_PASSWORD }))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16();
    eprintln!(
        "N4 concurrent statuses {statuses:?}; passwords checked {passwords_checked}; \
         attempts recorded {failures_recorded}; correct password afterwards: {locked}"
    );
    assert!(
        passwords_checked <= 5,
        "at most MAX_ATTEMPTS passwords may be checked per window, got {passwords_checked}"
    );
    assert!(
        failures_recorded >= 5,
        "the burst is recorded: {failures_recorded}"
    );
    assert!(
        statuses.iter().all(|s| [401, 429].contains(s)),
        "{statuses:?}"
    );
    assert_eq!(locked, 429, "the account is locked after the burst");
}
