#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! Server-observed SDK diagnostics: the revision each stream has been sent,
//! the per-environment convergence summary, and the poll-fallback and
//! playground counters on `/metrics`.

mod common;

use common::sdk::SimSdk;
use common::*;
use serde_json::{Value, json};
use std::time::Duration;

async fn get(client: &reqwest::Client, server: &Server, path: &str) -> Value {
    request(client, "GET", &server.url(path), None, 200).await
}

async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !check().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn delivered_revisions_converge_and_are_counted() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_sdk_diag").await;
    let metrics_addr = format!("127.0.0.1:{}", free_port());
    let server = Server::spawn(
        &db,
        &env.redis_url,
        &[("METRICS_ADDR", Some(&metrics_addr))],
    )
    .await;
    let (admin, sdk_key, env_id) = complete_setup(&server).await;
    let flags_url = server.url(&format!("/api/environments/{env_id}/flags"));
    request(
        &admin,
        "POST",
        &flags_url,
        Some(json!({ "key": "a", "is_enabled": true, "rollout_percentage": null, "description": null })),
        200,
    )
    .await;

    let sdk = SimSdk::start("diag", &server.base, &sdk_key);
    sdk.wait_readies(1, Duration::from_secs(15)).await;
    let latest = |v: &Value| v[0]["latest_revision"].as_i64();

    // After the replay, the stream has been sent the environment's latest revision.
    eventually("bootstrap delivered", || async {
        let envs = get(&admin, &server, "/api/health/environments").await;
        envs.as_array().unwrap().len() == 1
            && envs[0]["connections"] == 1
            && envs[0]["connections_behind"] == 0
            && latest(&envs).is_some()
    })
    .await;
    let envs = get(&admin, &server, "/api/health/environments").await;
    assert_eq!(envs[0]["environment_id"], env_id);
    let first = latest(&envs).unwrap();
    let conns = get(&admin, &server, "/api/health/connections").await;
    assert_eq!(conns[0]["last_delivered_revision"].as_i64(), Some(first));
    assert!(
        envs[0].get("client_ip").is_none(),
        "summary carries no addresses"
    );

    // A change raises both the environment's latest and the stream's delivered revision.
    request(
        &admin,
        "PATCH",
        &server.url(&format!("/api/environments/{env_id}/flags/a")),
        Some(json!({ "is_enabled": false })),
        200,
    )
    .await;
    eventually("change delivered", || async {
        let envs = get(&admin, &server, "/api/health/environments").await;
        latest(&envs).is_some_and(|l| l > first) && envs[0]["connections_behind"] == 0
    })
    .await;
    let envs = get(&admin, &server, "/api/health/environments").await;
    let conns = get(&admin, &server, "/api/health/connections").await;
    assert_eq!(
        conns[0]["last_delivered_revision"],
        envs[0]["latest_revision"]
    );

    // Poll fallback and playground usage are counted, with bounded labels only.
    for _ in 0..3 {
        let r = bearer_client(&sdk_key)
            .get(server.url("/flags/snapshot?v=2"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
    }
    let r = bearer_client(&sdk_key)
        .get(server.url("/flags/snapshot"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    for (key, status) in [("a", 200), ("missing", 404)] {
        let r = admin
            .post(server.url(&format!("/api/environments/{env_id}/flags/{key}/evaluate")))
            .header("X-Checkgate-Request", "true")
            .json(&json!({ "targeting_key": "u" }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), status);
    }
    let r = admin
        .post(server.url(&format!("/api/environments/{env_id}/flags/a/evaluate")))
        .header("X-Checkgate-Request", "true")
        .json(&json!({ "targeting_key": "u", "attributes": { "o": {} } }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);

    let metrics = reqwest::get(format!("http://{metrics_addr}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let line = |prefix: &str| {
        metrics
            .lines()
            .find(|l| l.starts_with(prefix))
            .unwrap_or_else(|| panic!("no metric line {prefix}"))
            .rsplit(' ')
            .next()
            .unwrap()
            .parse::<f64>()
            .unwrap()
    };
    assert!(line("checkgate_snapshot_requests_total{outcome=\"ok\",format=\"v2\"}") >= 3.0);
    assert!(line("checkgate_snapshot_requests_total{outcome=\"ok\",format=\"legacy\"}") >= 1.0);
    assert_eq!(
        line("checkgate_playground_evaluations_total{outcome=\"ok\"}"),
        1.0
    );
    assert_eq!(
        line("checkgate_playground_evaluations_total{outcome=\"not_found\"}"),
        1.0
    );
    assert_eq!(
        line("checkgate_playground_evaluations_total{outcome=\"invalid\"}"),
        1.0
    );
    // Never per-key, per-user or per-environment label values.
    for forbidden in [sdk_key.as_str(), env_id.as_str(), "targeting_key"] {
        assert!(!metrics.contains(forbidden), "metrics contain {forbidden}");
    }
    drop(sdk);
}

#[tokio::test]
async fn environment_summary_follows_project_membership() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_sdk_diag_rbac").await;
    let server = Server::spawn(&db, &env.redis_url, &[]).await;
    let (admin, sdk_key, _env_id) = complete_setup(&server).await;
    let sdk = SimSdk::start("diag", &server.base, &sdk_key);
    sdk.wait_readies(1, Duration::from_secs(15)).await;

    request(
        &admin,
        "POST",
        &server.url("/api/users"),
        Some(json!({ "name": "o", "email": "outsider@acme.test", "role": "viewer", "password": ADMIN_PASSWORD })),
        200,
    )
    .await;
    let outsider = login(&server, "outsider@acme.test", ADMIN_PASSWORD).await;
    eventually("admin sees the stream", || async {
        get(&admin, &server, "/api/health/environments")
            .await
            .as_array()
            .unwrap()
            .len()
            == 1
    })
    .await;
    assert_eq!(
        get(&outsider, &server, "/api/health/environments").await,
        json!([])
    );
    // SDK keys never reach it.
    let r = bearer_client(&sdk_key)
        .get(server.url("/api/health/environments"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    drop(sdk);
}
