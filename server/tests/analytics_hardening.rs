#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! Security finding N6 of the v0.1.31 review: analytics storage could grow
//! without bound (no retention; ingestion limited only per client address,
//! while SDK keys are public). Against real servers, PostgreSQL and Redis:
//!
//! - the per-credential ingest quota (rows and requests per minute) holds
//!   for one key used from many addresses, across instances and under
//!   concurrency, refuses whole batches, and falls back to per-instance
//!   enforcement when Redis is unreachable;
//! - batch and body limits refuse before anything is stored;
//! - retention deletes expired rows (and nothing else), keeps the rows of
//!   running experiments, runs once per interval across instances, never at
//!   startup, can be disabled, and stops promptly on shutdown.

mod common;

use common::cluster::Cluster;
use common::*;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Per-address limits off (tests send from one address) and a channel of
/// their own, so quota keys never collide with other tests on the same Redis.
fn base_env(channel: &str) -> Vec<(&'static str, Option<String>)> {
    vec![
        ("RATE_LIMIT_INGEST_PER_MINUTE", Some("0".into())),
        ("RATE_LIMIT_API_PER_MINUTE", Some("0".into())),
        (
            "REDIS_CHANNEL",
            Some(format!("checkgate_test_{channel}_{}", std::process::id())),
        ),
    ]
}

fn pairs<'a>(env: &'a [(&'static str, Option<String>)]) -> Vec<(&'a str, Option<&'a str>)> {
    env.iter().map(|(k, v)| (*k, v.as_deref())).collect()
}

fn impressions(n: usize, flag: &str) -> Value {
    Value::Array(
        (0..n)
            .map(|i| json!({ "flag_key": flag, "user_id": format!("u{i}"), "value": "true" }))
            .collect(),
    )
}

async fn ingest(
    client: &reqwest::Client,
    base: &str,
    env_id: &str,
    kind: &str,
    body: &Value,
) -> reqwest::Response {
    client
        .post(format!("{base}/api/environments/{env_id}/{kind}"))
        .header("X-Checkgate-Request", "true")
        .json(body)
        .send()
        .await
        .unwrap()
}

async fn count(pool: &sqlx::PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

async fn scrape(addr: &str) -> String {
    reqwest::get(format!("http://{addr}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

fn metric(text: &str, series: &str) -> f64 {
    text.lines()
        .find(|l| l.starts_with(series) && l[series.len()..].starts_with(' '))
        .unwrap_or_else(|| panic!("series {series} missing"))
        .rsplit(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

async fn new_key(admin: &reqwest::Client, base: &str, env_id: &str, name: &str) -> String {
    let projects = request(admin, "GET", &format!("{base}/api/projects"), None, 200).await;
    let project = projects[0]["id"].as_str().unwrap();
    let key = request(
        admin,
        "POST",
        &format!("{base}/api/projects/{project}/keys"),
        Some(json!({ "name": name, "environment_id": env_id })),
        200,
    )
    .await;
    key["key"].as_str().unwrap().to_string()
}

// ── Per-credential quota ─────────────────────────────────────────────────────

#[tokio::test]
async fn row_quota_is_per_key_and_refuses_whole_batches() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_n6_rows").await;
    let mut vars = base_env("n6_rows");
    vars.push(("RATE_LIMIT_INGEST_ROWS_PER_MINUTE", Some("1000".into())));
    vars.push(("METRICS_ADDR", Some("127.0.0.1:0".into())));
    let server = Server::spawn(&db, &env.redis_url, &pairs(&vars)).await;
    let metrics_addr = server.metrics_addr();
    let (admin, key, env_id) = complete_setup(&server).await;
    let pool = sqlx::PgPool::connect(&db).await.unwrap();
    let sdk = bearer_client(&key);

    // A 500-row batch costs 500 rows, not one request's worth.
    for _ in 0..2 {
        let r = ingest(
            &sdk,
            &server.base,
            &env_id,
            "impressions",
            &impressions(500, "f"),
        )
        .await;
        assert_eq!(r.status(), 204);
    }
    let r = ingest(
        &sdk,
        &server.base,
        &env_id,
        "impressions",
        &impressions(500, "f"),
    )
    .await;
    assert_eq!(r.status(), 429);
    let retry: u64 = r.headers()["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        (25..=31).contains(&retry),
        "500 rows refill in 30 s, got {retry}"
    );
    let text = r.text().await.unwrap();
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["retry_after_seconds"], retry);
    assert!(body["error"].is_string());
    assert!(
        !text.contains(&key) && !text.contains(&env_id),
        "no identifiers: {text}"
    );
    // Events share the credential's budget (100 rows: more than the few
    // that refill, at 60 ms per row, while these requests run).
    let r = ingest(
        &sdk,
        &server.base,
        &env_id,
        "events",
        &Value::Array(vec![json!({ "event_key": "goal", "user_id": "u1" }); 100]),
    )
    .await;
    assert_eq!(r.status(), 429, "events draw on the same row budget");
    // The refused batch stored nothing.
    assert_eq!(count(&pool, "SELECT count(*) FROM impressions").await, 1000);
    assert_eq!(count(&pool, "SELECT count(*) FROM events").await, 0);

    // Another key of the same environment has a budget of its own, and so
    // does a user ingesting with a session.
    let other = bearer_client(&new_key(&admin, &server.base, &env_id, "second").await);
    for client in [&other, &admin, &admin] {
        let r = ingest(
            client,
            &server.base,
            &env_id,
            "impressions",
            &impressions(500, "g"),
        )
        .await;
        assert_eq!(r.status(), 204);
    }
    assert_eq!(
        ingest(
            &admin,
            &server.base,
            &env_id,
            "impressions",
            &impressions(500, "g")
        )
        .await
        .status(),
        429,
        "sessions are charged too"
    );
    assert_eq!(count(&pool, "SELECT count(*) FROM impressions").await, 2500);

    let m = scrape(&metrics_addr).await;
    assert_eq!(
        metric(
            &m,
            "checkgate_analytics_rows_total{table=\"impressions\",outcome=\"accepted\"}"
        ),
        2500.0
    );
    assert_eq!(
        metric(
            &m,
            "checkgate_analytics_rows_total{table=\"impressions\",outcome=\"quota\"}"
        ),
        1000.0
    );
    assert_eq!(
        metric(
            &m,
            "checkgate_ingest_quota_rejections_total{limit=\"rows\"}"
        ),
        3.0
    );
    assert_eq!(
        metric(&m, "checkgate_ingest_quota_local_decisions_total"),
        0.0
    );
    assert!(
        !m.contains(&key) && !m.contains(&env_id),
        "no identifiers in metrics"
    );
    pool.close().await;
}

#[tokio::test]
async fn one_key_from_many_addresses_shares_its_quota() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_n6_addresses").await;
    let mut vars = base_env("n6_addresses");
    // Per-address limiting on (the default), behind a trusted proxy that
    // reports a different client for every request.
    vars.retain(|(k, _)| *k != "RATE_LIMIT_INGEST_PER_MINUTE");
    vars.push(("TRUSTED_PROXIES", Some("127.0.0.1/32".into())));
    vars.push(("RATE_LIMIT_INGEST_ROWS_PER_MINUTE", Some("2000".into())));
    let server = Server::spawn(&db, &env.redis_url, &pairs(&vars)).await;
    let (_, key, env_id) = complete_setup(&server).await;
    let sdk = bearer_client(&key);
    let mut statuses = Vec::new();
    for i in 0..6 {
        let r = sdk
            .post(server.url(&format!("/api/environments/{env_id}/impressions")))
            .header("X-Forwarded-For", format!("198.51.100.{i}"))
            .json(&impressions(500, "f"))
            .send()
            .await
            .unwrap();
        statuses.push(r.status().as_u16());
    }
    assert_eq!(
        statuses,
        [204, 204, 204, 204, 429, 429],
        "one budget for the key"
    );
    let pool = sqlx::PgPool::connect(&db).await.unwrap();
    assert_eq!(count(&pool, "SELECT count(*) FROM impressions").await, 2000);
    pool.close().await;
}

#[tokio::test]
async fn request_quota_counts_requests_of_any_size() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_n6_requests").await;
    let mut vars = base_env("n6_requests");
    vars.push(("RATE_LIMIT_INGEST_KEY_PER_MINUTE", Some("5".into())));
    let server = Server::spawn(&db, &env.redis_url, &pairs(&vars)).await;
    let (_, key, env_id) = complete_setup(&server).await;
    let sdk = bearer_client(&key);
    for _ in 0..5 {
        assert_eq!(
            ingest(
                &sdk,
                &server.base,
                &env_id,
                "impressions",
                &impressions(1, "f")
            )
            .await
            .status(),
            204
        );
    }
    assert_eq!(
        ingest(&sdk, &server.base, &env_id, "events", &json!([]))
            .await
            .status(),
        429,
        "an empty batch is still a request"
    );
}

#[tokio::test]
async fn quota_holds_across_instances_under_concurrency() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(
        &env,
        "cg_n6_cluster",
        2,
        &[("RATE_LIMIT_INGEST_ROWS_PER_MINUTE", Some("5000"))],
    )
    .await;
    let sdk = bearer_client(&c.sdk_key);
    let body = impressions(500, "f");
    let tasks: Vec<_> = (0..40)
        .map(|i| {
            let sdk = sdk.clone();
            let base = c.base(i % 2);
            let env_id = c.env_id.clone();
            let body = body.clone();
            tokio::spawn(async move {
                ingest(&sdk, &base, &env_id, "impressions", &body)
                    .await
                    .status()
                    .as_u16()
            })
        })
        .collect();
    let mut accepted = 0;
    let mut refused = 0;
    for t in tasks {
        match t.await.unwrap() {
            204 => accepted += 1,
            429 => refused += 1,
            other => panic!("unexpected status {other}"),
        }
    }
    // 5000 rows of burst = 10 batches; refill while the test runs adds at
    // most one more (83 rows per second).
    assert!(
        (10..=11).contains(&accepted),
        "accepted {accepted}, refused {refused} of 40 concurrent batches on 2 instances"
    );
    let stored = count(&c.pool, "SELECT count(*) FROM impressions").await;
    assert_eq!(stored, accepted * 500, "refused batches stored nothing");

    // Sequentially, alternating instances: still the same budget.
    let r0 = ingest(&sdk, &c.base(0), &c.env_id, "impressions", &body).await;
    let r1 = ingest(&sdk, &c.base(1), &c.env_id, "impressions", &body).await;
    assert_eq!((r0.status().as_u16(), r1.status().as_u16()), (429, 429));

    // Quota state is bounded: one hash per credential, expiring once refilled.
    let mut redis = redis::Client::open(env.redis_url.as_str())
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{}:ingest-quota:*", c.channel))
        .query_async(&mut redis)
        .await
        .unwrap();
    assert_eq!(keys.len(), 1, "{keys:?}");
    let ttl: i64 = redis::cmd("PTTL")
        .arg(&keys[0])
        .query_async(&mut redis)
        .await
        .unwrap();
    assert!((1..=62_000).contains(&ttl), "ttl {ttl} ms");
}

#[tokio::test]
async fn redis_outage_falls_back_to_a_per_instance_quota() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(
        &env,
        "cg_n6_outage",
        1,
        &[
            ("RATE_LIMIT_INGEST_ROWS_PER_MINUTE", Some("1000")),
            ("METRICS_ADDR", Some("127.0.0.1:0")),
        ],
    )
    .await;
    let metrics_addr = c.server(0).metrics_addr();
    let sdk = bearer_client(&c.sdk_key);
    c.redis(0).blackhole();
    c.redis(0).sever();
    let started = Instant::now();
    let mut statuses = Vec::new();
    for _ in 0..3 {
        statuses.push(
            ingest(
                &sdk,
                &c.base(0),
                &c.env_id,
                "impressions",
                &impressions(500, "f"),
            )
            .await
            .status()
            .as_u16(),
        );
    }
    assert_eq!(
        statuses,
        [204, 204, 429],
        "the same budget, enforced locally"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a Redis outage adds at most one short timeout: {:?}",
        started.elapsed()
    );
    let m = scrape(&metrics_addr).await;
    assert!(metric(&m, "checkgate_ingest_quota_local_decisions_total") >= 3.0);
    assert_eq!(
        count(&c.pool, "SELECT count(*) FROM impressions").await,
        1000
    );
    c.redis(0).heal();
}

#[tokio::test]
async fn oversized_batches_and_bodies_are_refused_before_storage() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_n6_sizes").await;
    let mut vars = base_env("n6_sizes");
    vars.push(("RATE_LIMIT_INGEST_ROWS_PER_MINUTE", Some("1000".into())));
    let server = Server::spawn(&db, &env.redis_url, &pairs(&vars)).await;
    let (admin, key, env_id) = complete_setup(&server).await;
    let pool = sqlx::PgPool::connect(&db).await.unwrap();
    let sdk = bearer_client(&key);

    // 501 rows: refused, and not charged.
    let r = ingest(
        &sdk,
        &server.base,
        &env_id,
        "impressions",
        &impressions(501, "f"),
    )
    .await;
    assert_eq!(r.status(), 413);
    // A body over 256 KiB: refused while reading, before parsing.
    let huge =
        json!([{ "flag_key": "f", "value": "true", "context": { "pad": "x".repeat(300_000) } }]);
    let r = ingest(&sdk, &server.base, &env_id, "impressions", &huge).await;
    assert_eq!(r.status(), 413);
    assert_eq!(count(&pool, "SELECT count(*) FROM impressions").await, 0);
    // Neither spent quota: a full budget is still available.
    for _ in 0..2 {
        assert_eq!(
            ingest(
                &sdk,
                &server.base,
                &env_id,
                "events",
                &Value::Array(vec![json!({ "event_key": "e" }); 500])
            )
            .await
            .status(),
            204
        );
    }
    assert_eq!(count(&pool, "SELECT count(*) FROM events").await, 1000);

    // Large rows are charged by size: one ~200 KiB row costs ~200 rows.
    let other = bearer_client(&new_key(&admin, &server.base, &env_id, "big-rows").await);
    let big =
        json!([{ "flag_key": "f", "value": "true", "context": { "pad": "x".repeat(200_000) } }]);
    let mut accepted = 0;
    for _ in 0..8 {
        if ingest(&other, &server.base, &env_id, "impressions", &big)
            .await
            .status()
            == 204
        {
            accepted += 1;
        }
    }
    assert_eq!(accepted, 5, "1000 units / 196 units per 200 KB row");
    pool.close().await;
}

#[tokio::test]
async fn invalid_credentials_cannot_ingest() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_n6_auth").await;
    let vars = base_env("n6_auth");
    let server = Server::spawn(&db, &env.redis_url, &pairs(&vars)).await;
    let (admin, key, env_id) = complete_setup(&server).await;
    let body = impressions(10, "f");
    let r = ingest(
        &bearer_client("sk_live_not_a_key"),
        &server.base,
        &env_id,
        "impressions",
        &body,
    )
    .await;
    assert_eq!(r.status(), 401);
    let r = ingest(
        &reqwest::Client::new(),
        &server.base,
        &env_id,
        "events",
        &json!([]),
    )
    .await;
    assert_eq!(r.status(), 401);
    // A key of one environment cannot report into another.
    let projects = request(&admin, "GET", &server.url("/api/projects"), None, 200).await;
    let other_env = request(
        &admin,
        "POST",
        &server.url(&format!(
            "/api/projects/{}/environments",
            projects[0]["id"].as_str().unwrap()
        )),
        Some(json!({ "name": "Other", "slug": "other" })),
        200,
    )
    .await;
    let other_env = other_env["id"].as_str().unwrap();
    let r = ingest(
        &bearer_client(&key),
        &server.base,
        other_env,
        "impressions",
        &body,
    )
    .await;
    assert_eq!(r.status(), 403);
    let pool = sqlx::PgPool::connect(&db).await.unwrap();
    assert_eq!(count(&pool, "SELECT count(*) FROM impressions").await, 0);
    pool.close().await;
}

// ── Retention ────────────────────────────────────────────────────────────────

async fn seed(pool: &sqlx::PgPool, env_id: &str, flag: &str, age_days: i32, n: i64) {
    sqlx::query(
        "INSERT INTO impressions (environment_id, flag_key, user_id, value, evaluated_at, received_at) \
         SELECT $1::uuid, $2, 'u' || g, 'true', NOW() - make_interval(days => $3), \
                NOW() - make_interval(days => $3) \
         FROM generate_series(1, $4) g",
    )
    .bind(env_id)
    .bind(flag)
    .bind(age_days)
    .bind(n)
    .execute(pool)
    .await
    .unwrap();
}

async fn wait_for(within: Duration, mut done: impl AsyncFnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if done().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

#[tokio::test]
async fn retention_deletes_expired_rows_only_and_reports_keep_working() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_n6_retention").await;
    let mut vars = base_env("n6_retention");
    vars.extend([
        ("ANALYTICS_RETENTION_DAYS", Some("30".into())),
        ("ANALYTICS_RETENTION_INTERVAL_SECS", Some("1".into())),
        ("ANALYTICS_RETENTION_BATCH_SIZE", Some("100".into())),
        ("METRICS_ADDR", Some("127.0.0.1:0".into())),
    ]);
    let server = Server::spawn(&db, &env.redis_url, &pairs(&vars)).await;
    let metrics_addr = server.metrics_addr();
    let (admin, key, env_id) = complete_setup(&server).await;
    let pool = sqlx::PgPool::connect(&db).await.unwrap();
    let base = server.url(&format!("/api/environments/{env_id}"));

    // Data that must survive: a flag (with its audit entry), an experiment
    // that is still running, and recent analytics.
    request(
        &admin,
        "POST",
        &format!("{base}/flags"),
        Some(json!({ "key": "exp-flag", "is_enabled": true, "rules": [] })),
        200,
    )
    .await;
    request(
        &admin,
        "POST",
        &format!("{base}/experiments"),
        Some(
            json!({ "key": "exp", "name": "Exp", "flag_key": "exp-flag", "goal_event_key": "buy" }),
        ),
        200,
    )
    .await;
    let audit_before = count(&pool, "SELECT count(*) FROM flag_audit_log").await;
    seed(&pool, &env_id, "old-flag", 31, 450).await;
    seed(&pool, &env_id, "exp-flag", 31, 40).await;
    seed(&pool, &env_id, "recent-flag", 29, 30).await;
    sqlx::query(
        "INSERT INTO events (environment_id, event_key, user_id, received_at) \
         SELECT $1::uuid, k, 'u' || g, NOW() - INTERVAL '40 days' \
         FROM generate_series(1, 20) g, (VALUES ('buy'), ('other')) v(k)",
    )
    .bind(&env_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        ingest(
            &bearer_client(&key),
            &server.base,
            &env_id,
            "impressions",
            &impressions(5, "live")
        )
        .await
        .status(),
        204
    );

    assert!(
        wait_for(Duration::from_secs(15), async || {
            count(
                &pool,
                "SELECT count(*) FROM impressions WHERE flag_key = 'old-flag'",
            )
            .await
                == 0
        })
        .await,
        "expired impressions are deleted"
    );
    assert!(
        wait_for(Duration::from_secs(5), async || {
            count(
                &pool,
                "SELECT count(*) FROM events WHERE event_key = 'other'",
            )
            .await
                == 0
        })
        .await
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM impressions WHERE flag_key = 'exp-flag'"
        )
        .await,
        40
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM events WHERE event_key = 'buy'").await,
        20
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM impressions WHERE flag_key = 'recent-flag'"
        )
        .await,
        30
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM impressions WHERE flag_key = 'live'"
        )
        .await,
        5
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM flag_audit_log").await,
        audit_before
    );
    assert_eq!(count(&pool, "SELECT count(*) FROM experiments").await, 1);
    assert_eq!(count(&pool, "SELECT count(*) FROM flags").await, 1);

    // Reports answer from what is retained.
    let stats = request(
        &admin,
        "GET",
        &format!("{base}/impressions/stats"),
        None,
        200,
    )
    .await;
    let flags: Vec<&str> = stats
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["flag_key"].as_str().unwrap())
        .collect();
    assert!(
        !flags.contains(&"old-flag") && flags.contains(&"exp-flag"),
        "{flags:?}"
    );
    let results = request(
        &admin,
        "GET",
        &format!("{base}/experiments/exp/results"),
        None,
        200,
    )
    .await;
    assert_eq!(results["variants"][0]["exposed"], 40, "{results}");
    request(
        &admin,
        "GET",
        &format!("{base}/impressions/exposure?flag_key=exp-flag&days=90"),
        None,
        200,
    )
    .await;
    request(&admin, "GET", &format!("{base}/events/keys"), None, 200).await;

    let m = scrape(&metrics_addr).await;
    assert!(
        metric(
            &m,
            "checkgate_analytics_cleanup_runs_total{outcome=\"success\"}"
        ) >= 1.0
    );
    assert_eq!(
        metric(
            &m,
            "checkgate_analytics_cleanup_runs_total{outcome=\"failure\"}"
        ),
        0.0
    );
    assert_eq!(
        metric(
            &m,
            "checkgate_analytics_cleanup_deleted_rows_total{table=\"impressions\"}"
        ),
        450.0
    );
    assert_eq!(
        metric(
            &m,
            "checkgate_analytics_cleanup_deleted_rows_total{table=\"events\"}"
        ),
        20.0
    );
    assert!(
        metric(
            &m,
            "checkgate_analytics_cleanup_last_success_timestamp_seconds"
        ) > 1.7e9
    );
    pool.close().await;
}

#[tokio::test]
async fn no_deletion_at_startup_and_zero_keeps_everything() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_n6_startup").await;
    let mut vars = base_env("n6_startup");
    vars.push(("ANALYTICS_RETENTION_DAYS", Some("0".into())));
    vars.push(("ANALYTICS_RETENTION_INTERVAL_SECS", Some("1".into())));
    let server = Server::spawn(&db, &env.redis_url, &pairs(&vars)).await;
    let (_, _, env_id) = complete_setup(&server).await;
    let pool = sqlx::PgPool::connect(&db).await.unwrap();
    seed(&pool, &env_id, "old", 400, 100).await;

    // Disabled: nothing is ever deleted.
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(count(&pool, "SELECT count(*) FROM impressions").await, 100);
    assert!(
        server.log().contains("kept forever"),
        "disabled mode is logged"
    );

    // Upgrade with the default schedule: the first check is a minute after
    // startup, so a restart never deletes synchronously.
    let mut upgraded = base_env("n6_startup");
    upgraded.push(("ANALYTICS_RETENTION_DAYS", Some("30".into())));
    let server = server.restart(&db, &env.redis_url, &pairs(&upgraded)).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(count(&pool, "SELECT count(*) FROM impressions").await, 100);

    // With a short interval the backlog goes.
    let mut fast = upgraded.clone();
    fast.push(("ANALYTICS_RETENTION_INTERVAL_SECS", Some("1".into())));
    let _server = server.restart(&db, &env.redis_url, &pairs(&fast)).await;
    assert!(
        wait_for(Duration::from_secs(15), async || {
            count(&pool, "SELECT count(*) FROM impressions").await == 0
        })
        .await
    );
    pool.close().await;
}

#[tokio::test]
async fn invalid_retention_and_quota_settings_stop_startup() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_n6_config").await;
    for (name, value) in [
        ("ANALYTICS_RETENTION_DAYS", "-5"),
        ("ANALYTICS_RETENTION_DAYS", "forever"),
        ("ANALYTICS_RETENTION_INTERVAL_SECS", "0"),
        ("ANALYTICS_RETENTION_BATCH_SIZE", "10"),
        ("RATE_LIMIT_INGEST_ROWS_PER_MINUTE", "100"),
        ("RATE_LIMIT_INGEST_KEY_PER_MINUTE", "x"),
    ] {
        let log = expect_startup_failure(&db, &env.redis_url, &[(name, Some(value))]).await;
        assert!(log.contains(name), "{name}={value}: {log}");
    }
}

#[tokio::test]
async fn two_instances_share_one_cleanup_schedule() {
    let Some(env) = test_env() else { return };
    let cluster_started = Instant::now();
    let c = Cluster::start(
        &env,
        "cg_n6_two",
        2,
        &[
            ("ANALYTICS_RETENTION_DAYS", Some("30")),
            ("ANALYTICS_RETENTION_INTERVAL_SECS", Some("2")),
            ("ANALYTICS_RETENTION_BATCH_SIZE", Some("100")),
        ],
    )
    .await;
    seed(&c.pool, &c.env_id, "old", 60, 1234).await;
    assert!(
        wait_for(Duration::from_secs(15), async || {
            count(&c.pool, "SELECT count(*) FROM impressions").await == 0
        })
        .await
    );
    // Keep both instances checking for a while longer.
    tokio::time::sleep(Duration::from_secs(8)).await;
    // Every pass is logged, including empty ones before the seed.
    let elapsed = cluster_started.elapsed().as_secs();

    let mut passes = 0;
    let mut deleted = 0;
    let mut by_instance = [0; 2];
    for (i, (_, log)) in c.logs().iter().enumerate() {
        for line in log
            .lines()
            .filter(|l| l.contains("Analytics retention pass complete"))
        {
            passes += 1;
            by_instance[i] += 1;
            let v: Value = serde_json::from_str(line).unwrap();
            deleted += v["fields"]["deleted_impressions"].as_i64().unwrap();
        }
    }
    assert_eq!(
        deleted, 1234,
        "every row deleted exactly once across instances"
    );
    // At most one pass per 2 s interval cluster-wide (+1 for the edges);
    // uncoordinated instances would run about twice as many.
    let allowed = usize::try_from(elapsed.div_ceil(2)).unwrap() + 1;
    assert!(
        passes <= allowed,
        "{passes} passes in {elapsed} s (allowed {allowed}); per instance {by_instance:?}"
    );
}

#[tokio::test]
async fn shutdown_stops_a_long_pass_and_a_restart_resumes_it() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_n6_shutdown").await;
    let mut vars = base_env("n6_shutdown");
    vars.extend([
        ("ANALYTICS_RETENTION_DAYS", Some("30".into())),
        ("ANALYTICS_RETENTION_INTERVAL_SECS", Some("1".into())),
        ("ANALYTICS_RETENTION_BATCH_SIZE", Some("100".into())),
    ]);
    let mut server = Server::spawn(&db, &env.redis_url, &pairs(&vars)).await;
    let (_, _, env_id) = complete_setup(&server).await;
    let pool = sqlx::PgPool::connect(&db).await.unwrap();
    // 100 batches of 100 with a 50 ms pause: several seconds of work.
    seed(&pool, &env_id, "old", 60, 10_000).await;
    assert!(
        wait_for(Duration::from_secs(10), async || {
            count(&pool, "SELECT count(*) FROM impressions").await < 10_000
        })
        .await,
        "the pass started"
    );
    server.terminate();
    let status = server
        .wait_exit(Duration::from_secs(10))
        .await
        .expect("exits promptly during a pass");
    assert!(status.success(), "{status:?}");
    let left = count(&pool, "SELECT count(*) FROM impressions").await;
    assert!(left > 0, "interrupted before the end");
    assert_eq!(left % 100, 0, "{left}: only whole batches were deleted");
    assert!(server.log().contains("interrupted by shutdown"));

    let _server = server.restart(&db, &env.redis_url, &pairs(&vars)).await;
    assert!(
        wait_for(Duration::from_secs(30), async || {
            count(&pool, "SELECT count(*) FROM impressions").await == 0
        })
        .await,
        "the next instance resumes the pass"
    );
    pool.close().await;
}
