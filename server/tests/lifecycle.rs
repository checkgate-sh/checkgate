#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! Health, readiness, graceful shutdown, metrics and admission control,
//! against real server processes, PostgreSQL and Redis.

mod common;

use common::chaos::ChaosProxy;
use common::cluster::{Cluster, flag_body};
use common::sdk::SimSdk;
use common::{free_port, request, test_env};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const CONVERGE: Duration = Duration::from_secs(20);

async fn get(url: &str) -> (u16, Value) {
    let r = reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

async fn wait_for_status(url: &str, want: u16, within: Duration) -> Value {
    let deadline = Instant::now() + within;
    loop {
        let (status, body) = get(url).await;
        if status == want {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "{url}: still {status} {body}, wanted {want}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// ── Liveness and readiness ───────────────────────────────────────────────────

#[tokio::test]
async fn readiness_follows_dependencies_and_liveness_does_not() {
    let Some(env) = test_env() else { return };
    // The instance reaches PostgreSQL through a proxy so the database can be
    // taken away from it alone.
    let db_url = env.fresh_database("life_readiness").await;
    let (pg, pg_url) = ChaosProxy::for_url(&db_url).await;
    let (redis, redis_url) = ChaosProxy::for_url(&env.redis_url).await;
    let server = common::Server::spawn(
        &pg_url,
        &redis_url,
        &[
            ("REDIS_PING_INTERVAL_SECS", Some("1")),
            ("REDIS_CHANNEL", Some("life_readiness")),
        ],
    )
    .await;

    assert_eq!(
        get(&server.url("/health/live")).await,
        (200, json!({"status": "alive"}))
    );
    let ready = wait_for_status(&server.url("/health/ready"), 200, CONVERGE).await;
    assert_eq!(ready["status"], "ready");
    assert_eq!(ready["checks"], json!({"database": "ok", "redis": "ok"}));

    // Database gone: not ready, still alive.
    pg.refuse();
    let body = wait_for_status(&server.url("/health/ready"), 503, CONVERGE).await;
    assert_eq!(body["checks"]["database"], "unavailable");
    assert_eq!(get(&server.url("/health/live")).await.0, 200);
    assert_eq!(
        get(&server.url("/health")).await.0,
        200,
        "legacy /health unchanged"
    );
    pg.heal();
    wait_for_status(&server.url("/health/ready"), 200, CONVERGE).await;

    // Redis gone: degraded but in service (PostgreSQL is authoritative).
    redis.refuse();
    let deadline = Instant::now() + CONVERGE;
    loop {
        let (status, body) = get(&server.url("/health/ready")).await;
        assert_eq!(
            status, 200,
            "Redis loss must not fail readiness by default: {body}"
        );
        if body["status"] == "degraded" {
            assert_eq!(body["checks"]["redis"], "unavailable");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "readiness never reported degraded"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    redis.heal();
    let body = wait_for_status(&server.url("/health/ready"), 200, CONVERGE).await;
    let deadline = Instant::now() + CONVERGE;
    let mut body = body;
    while body["status"] != "ready" {
        assert!(
            Instant::now() < deadline,
            "readiness did not recover: {body}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        body = get(&server.url("/health/ready")).await.1;
    }
}

#[tokio::test]
async fn ready_requires_redis_when_configured() {
    let Some(env) = test_env() else { return };
    let db_url = env.fresh_database("life_ready_redis").await;
    let (redis, redis_url) = ChaosProxy::for_url(&env.redis_url).await;
    let server = common::Server::spawn(
        &db_url,
        &redis_url,
        &[
            ("READY_REQUIRES_REDIS", Some("true")),
            ("REDIS_PING_INTERVAL_SECS", Some("1")),
            ("REDIS_CHANNEL", Some("life_ready_redis")),
        ],
    )
    .await;
    wait_for_status(&server.url("/health/ready"), 200, CONVERGE).await;
    redis.refuse();
    wait_for_status(&server.url("/health/ready"), 503, CONVERGE).await;
    assert_eq!(get(&server.url("/health/live")).await.0, 200);
    redis.heal();
    wait_for_status(&server.url("/health/ready"), 200, CONVERGE).await;
}

// ── Shutdown ─────────────────────────────────────────────────────────────────

/// SIGTERM with streams open: readiness fails during the drain delay while
/// requests are still served, new streams are refused, open streams are
/// closed, and the process exits cleanly well within the grace period.
#[tokio::test]
async fn sigterm_drains_closes_streams_and_exits_within_grace() {
    let Some(env) = test_env() else { return };
    let mut c = Cluster::start(
        &env,
        "life_sigterm",
        2,
        &[
            ("SHUTDOWN_DRAIN_DELAY_SECS", Some("2")),
            ("SHUTDOWN_GRACE_PERIOD_SECS", Some("10")),
        ],
    )
    .await;
    c.create_flag(0, "drain", false).await;
    let sdk = c.sdk("sdk-on-a", 0);
    let other = c.sdk("sdk-on-b", 1);
    c.wait_converged(&[&sdk, &other], &c.env_id, CONVERGE).await;

    let started = Instant::now();
    c.server(0).terminate();
    let body = wait_for_status(
        &format!("{}/health/ready", c.base(0)),
        503,
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(body["status"], "draining");
    // Still serving during the drain delay; new streams are turned away.
    assert_eq!(get(&format!("{}/health/live", c.base(0))).await.0, 200);
    let refused = reqwest::Client::new()
        .get(format!("{}/stream", c.base(0)))
        .bearer_auth(&c.sdk_key)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 503);
    assert!(refused.headers().contains_key("retry-after"));

    let status = c
        .server_mut(0)
        .wait_exit(Duration::from_secs(15))
        .await
        .expect("process exits within the grace period");
    assert!(status.success(), "clean exit: {status:?}");
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(2),
        "drain delay honoured ({took:?})"
    );
    assert!(
        took < Duration::from_secs(10),
        "exited before the grace period ran out ({took:?})"
    );
    let log = c.server(0).log();
    assert!(
        log.contains("\"reason\":\"shutdown\""),
        "streams closed for shutdown"
    );
    assert!(log.contains("Server shut down gracefully"));
    assert!(
        !log.contains("Grace period elapsed"),
        "nothing had to be cut off"
    );

    // The client of the stopped instance is not stranded: a change made on
    // the surviving instance reaches the other client, and the restarted
    // instance's client re-bootstraps.
    c.set_enabled(1, "drain", true).await;
    c.wait_converged(&[&other], &c.env_id, CONVERGE).await;
    c.kill(0);
    c.start_instance(0).await;
    c.wait_converged(&[&sdk, &other], &c.env_id, CONVERGE).await;
}

// ── Metrics ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn metrics_endpoint_is_valid_bounded_and_private() {
    let Some(env) = test_env() else { return };
    let metrics_port = free_port();
    let addr = format!("127.0.0.1:{metrics_port}");
    let c = Cluster::start(&env, "life_metrics", 1, &[("METRICS_ADDR", Some(&addr))]).await;
    c.create_flag(0, "metric-flag-key", true).await;
    let sdk = c.sdk("sdk", 0);
    c.wait_converged(&[&sdk], &c.env_id, CONVERGE).await;
    c.set_enabled(0, "metric-flag-key", false).await;
    c.wait_converged(&[&sdk], &c.env_id, CONVERGE).await;

    let resp = reqwest::get(format!("http://{addr}/metrics"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/openmetrics-text"),
        "OpenMetrics content type"
    );
    let text = resp.text().await.unwrap();
    assert!(text.trim_end().ends_with("# EOF"), "OpenMetrics terminator");

    for (name, kind) in [
        ("checkgate_http_requests", "counter"),
        ("checkgate_http_request_duration_seconds", "histogram"),
        ("checkgate_flag_updates", "counter"),
        ("checkgate_flag_update_publish_failures", "counter"),
        ("checkgate_flag_updates_received", "counter"),
        ("checkgate_flag_publish_duration_seconds", "histogram"),
        ("checkgate_redis_connected", "gauge"),
        ("checkgate_redis_reconnections", "counter"),
        ("checkgate_resync_requests", "counter"),
        ("checkgate_resync_failures", "counter"),
        ("checkgate_resync_pending", "gauge"),
        ("checkgate_sync_reconciliations", "counter"),
        ("checkgate_sync_reconcile_duration_seconds", "histogram"),
        ("checkgate_sync_divergences", "counter"),
        ("checkgate_sync_tracked_flags", "gauge"),
        ("checkgate_sse_connections", "counter"),
        ("checkgate_sse_rejections", "counter"),
        ("checkgate_sse_disconnects", "counter"),
        ("checkgate_sse_lagged_messages", "counter"),
        ("checkgate_sse_active_connections", "gauge"),
        ("checkgate_sse_bootstrap_duration_seconds", "histogram"),
        ("checkgate_snapshot_query_duration_seconds", "histogram"),
        ("checkgate_db_pool_connections", "gauge"),
        ("checkgate_db_pool_idle_connections", "gauge"),
        ("checkgate_db_pool_max_connections", "gauge"),
        ("checkgate_database_up", "gauge"),
        ("checkgate_ready", "gauge"),
        ("checkgate_build", "info"),
    ] {
        assert!(
            text.contains(&format!("# TYPE {name} {kind}\n")),
            "TYPE for {name}"
        );
        assert!(text.contains(&format!("# HELP {name} ")), "HELP for {name}");
    }

    let value = |series: &str| -> f64 {
        text.lines()
            .find(|l| l.starts_with(series) && l[series.len()..].starts_with(' '))
            .unwrap_or_else(|| panic!("series {series} missing"))
            .rsplit(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    assert_eq!(value("checkgate_sse_active_connections"), 1.0);
    assert_eq!(value("checkgate_redis_connected"), 1.0);
    assert_eq!(value("checkgate_ready"), 1.0);
    assert_eq!(value("checkgate_database_up"), 1.0);
    assert!(value("checkgate_flag_updates_total") >= 2.0);
    assert!(value("checkgate_flag_updates_received_total") >= 2.0);
    assert!(value("checkgate_sync_tracked_flags") >= 1.0);
    assert!(value("checkgate_sync_reconciliations_total{outcome=\"success\"}") >= 1.0);
    assert!(text.contains(
        "checkgate_http_requests_total{method=\"PATCH\",route=\"/api/environments/{env_id}/flags/{key}\",status_class=\"2xx\"}"
    ));

    // No identifiers in any label value.
    for secret in [&c.env_id, &c.sdk_key, "metric-flag-key", "127.0.0.1"] {
        assert!(!text.contains(secret), "metrics expose {secret}");
    }
    let series = text.lines().filter(|l| !l.starts_with('#')).count();
    assert!(series < 600, "series count stays bounded ({series})");

    // Not on the public port.
    let public = reqwest::get(format!("{}/metrics", c.base(0)))
        .await
        .unwrap();
    assert!(
        !public
            .text()
            .await
            .unwrap()
            .contains("checkgate_http_requests")
    );
}

// ── Admission control ────────────────────────────────────────────────────────

#[tokio::test]
async fn stream_capacity_refuses_with_retry_after_and_recovers() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(
        &env,
        "life_capacity",
        1,
        &[("SSE_MAX_CONNECTIONS", Some("2"))],
    )
    .await;
    let a = c.sdk("a", 0);
    let b = c.sdk("b", 0);
    a.wait_readies(1, CONVERGE).await;
    b.wait_readies(1, CONVERGE).await;
    let refused = reqwest::Client::new()
        .get(format!("{}/stream", c.base(0)))
        .bearer_auth(&c.sdk_key)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 503);
    let retry: u64 = refused.headers()["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=10).contains(&retry));
    // The snapshot fallback still works at stream capacity.
    let (status, _) = {
        let r = reqwest::Client::new()
            .get(format!("{}/flags/snapshot", c.base(0)))
            .bearer_auth(&c.sdk_key)
            .send()
            .await
            .unwrap();
        (r.status().as_u16(), ())
    };
    assert_eq!(status, 200);

    drop(a);
    let late = c.sdk("late", 0);
    late.wait_readies(1, CONVERGE).await;
}

// ── Static files ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn dashboard_files_get_security_headers() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(&env, "life_static", 1, &[]).await;
    std::fs::write(
        c.instances[0].dir.join("index.html"),
        "<!doctype html><title>t</title>",
    )
    .unwrap();
    std::fs::create_dir_all(c.instances[0].dir.join("assets")).unwrap();
    std::fs::write(c.instances[0].dir.join("assets/app.js"), "console.log(1)").unwrap();
    for path in ["/", "/flags", "/assets/app.js"] {
        let r = reqwest::get(format!("{}{path}", c.base(0))).await.unwrap();
        assert_eq!(r.status(), 200, "{path}");
        let h = r.headers();
        assert_eq!(h["x-content-type-options"], "nosniff", "{path}");
        assert_eq!(h["x-frame-options"], "DENY", "{path}");
        let csp = h["content-security-policy"].to_str().unwrap();
        assert!(
            csp.contains("https://fonts.googleapis.com"),
            "{path}: dashboard CSP"
        );
        assert!(csp.contains("frame-ancestors 'none'"), "{path}");
    }
    let api = reqwest::get(format!("{}/health/live", c.base(0)))
        .await
        .unwrap();
    let csp = api.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        !csp.contains("fonts.googleapis.com"),
        "API responses keep the strict policy"
    );
}

// ── Reconciliation precision ─────────────────────────────────────────────────

/// Changes that are relayed normally — including archival, which has no
/// change event, and segment edits, which re-revision flags — never look like
/// lost notifications.
#[tokio::test]
async fn relayed_changes_archival_and_segment_edits_cause_no_resync() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(&env, "life_no_false_resync", 2, &[]).await;
    let segs = format!("{}/api/environments/{}/segments", c.base(0), c.env_id);
    request(
        &c.admin,
        "POST",
        &segs,
        Some(json!({ "key": "staff", "name": "Staff",
            "rules": [{ "attribute": "email", "operator": "ends_with", "values": ["@acme.test"] }] })),
        200,
    )
    .await;
    let mut body = flag_body("seg-flag", true);
    body["rules"] = json!([{ "segment_key": "staff" }]);
    c.create_flag_body(0, &c.env_id.clone(), body).await;
    c.create_flag(1, "plain", false).await;
    let sdks = [c.sdk("a", 0), c.sdk("b", 1)];
    let all: Vec<&SimSdk> = sdks.iter().collect();
    c.wait_converged(&all, &c.env_id, CONVERGE).await;

    let flags = format!("{}/api/environments/{}/flags", c.base(1), c.env_id);
    request(
        &c.admin,
        "POST",
        &format!("{flags}/plain/archive"),
        None,
        200,
    )
    .await;
    request(
        &c.admin,
        "PATCH",
        &format!("{segs}/staff"),
        Some(json!({ "rules": [{ "attribute": "email", "operator": "ends_with", "values": ["@corp.test"] }] })),
        200,
    )
    .await;
    c.set_enabled(1, "plain", true).await;
    let t = c.wait_converged(&all, &c.env_id, CONVERGE).await;
    c.record("segment edit re-revisions flag", t);
    // Expanded rules reached the clients.
    let rules = sdks[1].snapshot().0["seg-flag"]["rules"].to_string();
    assert!(
        rules.contains("@corp.test"),
        "segment change expanded: {rules}"
    );

    // Several reconciliation passes (1 s interval, 2 s grace).
    tokio::time::sleep(Duration::from_secs(5)).await;
    for sdk in &sdks {
        assert_eq!(
            sdk.with(|s| s.connects),
            1,
            "{} was made to reconnect",
            sdk.name
        );
    }
    for (port, log) in c.logs() {
        assert!(
            !log.contains("never relayed"),
            "false divergence on :{port}"
        );
    }
}

/// A lost change in one environment re-bootstraps that environment's clients
/// only.
#[tokio::test]
async fn reconciliation_resyncs_only_the_affected_environment() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(&env, "life_targeted_resync", 1, &[]).await;
    let project: String =
        sqlx::query_scalar("SELECT project_id::text FROM environments WHERE id = $1::uuid")
            .bind(&c.env_id)
            .fetch_one(&c.pool)
            .await
            .unwrap();
    let env2 = request(
        &c.admin,
        "POST",
        &format!("{}/api/projects/{project}/environments", c.base(0)),
        Some(json!({ "name": "Other", "slug": "other-env" })),
        200,
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let key2 = request(
        &c.admin,
        "POST",
        &format!("{}/api/projects/{project}/keys", c.base(0)),
        Some(json!({ "name": "other", "environment_id": env2 })),
        200,
    )
    .await["key"]
        .as_str()
        .unwrap()
        .to_string();
    c.create_flag(0, "f", false).await;
    c.create_flag_body(0, &env2, flag_body("f", false)).await;
    let affected = c.sdk("affected", 0);
    let bystander = SimSdk::start("bystander", &c.base(0), &key2);
    c.wait_converged(&[&affected], &c.env_id, CONVERGE).await;
    c.wait_converged(&[&bystander], &env2, CONVERGE).await;

    sqlx::query(
        "UPDATE flags SET data = jsonb_set(data, '{is_enabled}', 'true') \
         WHERE key = 'f' AND environment_id = $1::uuid",
    )
    .bind(&c.env_id)
    .execute(&c.pool)
    .await
    .unwrap();
    let t = c.wait_converged(&[&affected], &c.env_id, CONVERGE).await;
    c.record("targeted resync", t);
    assert_eq!(
        affected.with(|s| s.connects),
        2,
        "affected client re-bootstrapped"
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        bystander.with(|s| s.connects),
        1,
        "other environment untouched"
    );
}
