#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! The evaluation playground (`POST /api/environments/{env}/flags/{key}/evaluate`):
//! it answers what the SDKs answer for the same configuration revision, it
//! respects project RBAC, it validates its input, and it changes nothing.

mod common;

use common::*;
use serde_json::{Value, json};

fn vectors() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../sdks/conformance/vectors/evaluation.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

async fn evaluate(
    client: &reqwest::Client,
    server: &Server,
    env: &str,
    key: &str,
    body: Value,
) -> reqwest::Response {
    client
        .post(server.url(&format!("/api/environments/{env}/flags/{key}/evaluate")))
        .header("X-Checkgate-Request", "true")
        .json(&body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn playground_matches_the_shared_vectors_and_changes_nothing() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_playground").await;
    let server = Server::spawn(&db, &env.redis_url, &[]).await;
    let (admin, sdk_key, env_id) = complete_setup(&server).await;

    let doc = vectors();
    for flag in doc["flags"].as_array().unwrap() {
        request(
            &admin,
            "POST",
            &server.url(&format!("/api/environments/{env_id}/flags")),
            Some(flag.clone()),
            200,
        )
        .await;
    }

    // Revisions as the SDK snapshot serves them.
    let snapshot: Value = bearer_client(&sdk_key)
        .get(server.url("/flags/snapshot?v=2"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let audit_before = request(
        &admin,
        "GET",
        &server.url(&format!("/api/environments/{env_id}/audit")),
        None,
        200,
    )
    .await;

    let mut checked = 0;
    for case in doc["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let flag = case["flag"].as_str().unwrap();
        let r = evaluate(
            &admin,
            &server,
            &env_id,
            flag,
            json!({ "targeting_key": case["user"], "attributes": case["attributes"] }),
        )
        .await;
        if case["expect"].is_null() {
            assert_eq!(r.status(), 404, "{name}");
            continue;
        }
        assert_eq!(r.status(), 200, "{name}");
        let got: Value = r.json().await.unwrap();
        let want = &case["expect"];
        for field in ["enabled", "value", "reason"] {
            assert_eq!(got[field], want[field], "{name}: {field}");
        }
        assert_eq!(
            got["matched_rule"]
                .get("source_index")
                .cloned()
                .unwrap_or(Value::Null),
            want.get("rule_index").cloned().unwrap_or(Value::Null),
            "{name}: rule (no segments, so expanded index == stored index)"
        );
        assert_eq!(
            got["rollout"].get("bucket").cloned().unwrap_or(Value::Null),
            want.get("rollout_bucket").cloned().unwrap_or(Value::Null),
            "{name}: bucket"
        );
        assert_eq!(
            got["variant"].get("index").cloned().unwrap_or(Value::Null),
            want.get("variant_index").cloned().unwrap_or(Value::Null),
            "{name}: variant"
        );
        assert_eq!(
            got["revision"], snapshot["revisions"][flag],
            "{name}: revision"
        );
        checked += 1;
    }
    assert!(checked > 80, "checked {checked}");

    // Prerequisites are reported with their outcome and revision.
    let r = evaluate(
        &admin,
        &server,
        &env_id,
        "needs-chain",
        json!({ "targeting_key": "u" }),
    )
    .await;
    let got: Value = r.json().await.unwrap();
    assert_eq!(got["reason"], "PREREQUISITE_FAILED");
    assert_eq!(got["prerequisites"][0]["flag_key"], "static-on");
    assert_eq!(got["prerequisites"][0]["outcome"], "satisfied");
    assert_eq!(got["prerequisites"][1]["outcome"], "unsatisfied");
    assert_eq!(
        got["prerequisites"][1]["revision"],
        snapshot["revisions"]["needs-base"]
    );
    assert_eq!(got["referenced_attributes"], json!(["beta"]));
    assert_eq!(got["missing_attributes"], json!(["beta"]));

    // Nothing changed: same revisions, no audit entries, no impressions.
    let snapshot_after: Value = bearer_client(&sdk_key)
        .get(server.url("/flags/snapshot?v=2"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(snapshot["revisions"], snapshot_after["revisions"]);
    let audit_after = request(
        &admin,
        "GET",
        &server.url(&format!("/api/environments/{env_id}/audit")),
        None,
        200,
    )
    .await;
    assert_eq!(audit_before, audit_after);
    let stats = request(
        &admin,
        "GET",
        &server.url(&format!("/api/environments/{env_id}/impressions?limit=10")),
        None,
        200,
    )
    .await;
    assert_eq!(stats["total"], 0, "{stats}");
}

#[tokio::test]
async fn playground_traces_segment_rules_and_validates_input() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_playground_seg").await;
    let server = Server::spawn(&db, &env.redis_url, &[]).await;
    let (admin, sdk_key, env_id) = complete_setup(&server).await;

    request(
        &admin,
        "POST",
        &server.url(&format!("/api/environments/{env_id}/segments")),
        Some(json!({
            "name": "Beta", "key": "beta",
            "rules": [
                { "attribute": "email", "operator": "ends_with", "values": ["@acme.test"] },
                { "attribute": "tier", "operator": "equals", "values": ["beta"] }
            ]
        })),
        200,
    )
    .await;
    request(
        &admin,
        "POST",
        &server.url(&format!("/api/environments/{env_id}/flags")),
        Some(json!({
            "key": "theme", "is_enabled": true, "rollout_percentage": 0, "description": null,
            "flag_type": "string", "default_value": "classic",
            "rules": [
                { "attribute": "country", "operator": "equals", "values": ["PH"], "variant": "local" },
                { "segment_key": "beta", "variant": "beta" }
            ]
        })),
        200,
    )
    .await;

    // The second segment rule is expanded rule 2, stored rule 1.
    let r = evaluate(
        &admin,
        &server,
        &env_id,
        "theme",
        json!({ "targeting_key": "u", "attributes": { "tier": "beta" } }),
    )
    .await;
    assert_eq!(r.status(), 200);
    let got: Value = r.json().await.unwrap();
    assert_eq!(got["value"], "beta");
    assert_eq!(got["reason"], "TARGETING_MATCH");
    assert_eq!(got["matched_rule"]["source_index"], 1);
    assert_eq!(got["matched_rule"]["segment_key"], "beta");
    assert_eq!(got["matched_rule"]["segment_rule_index"], 1);
    assert_eq!(got["matched_rule"]["rule"]["attribute"], "tier");
    assert_eq!(
        got["referenced_attributes"],
        json!(["country", "email", "tier"])
    );
    assert_eq!(got["missing_attributes"], json!(["country", "email"]));

    // Numbers and booleans are accepted as strings.
    let r = evaluate(
        &admin,
        &server,
        &env_id,
        "theme",
        json!({ "targeting_key": "u", "attributes": { "country": "PH", "age": 30, "vip": true } }),
    )
    .await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Value>().await.unwrap()["value"], "local");

    // Invalid input is refused with a message, never half-evaluated.
    for bad in [
        json!({ "targeting_key": "u", "attributes": { "o": { "a": 1 } } }),
        json!({ "targeting_key": "u", "attributes": { "n": null } }),
        json!({ "targeting_key": "u", "attributes": [1, 2] }),
        json!({ "attributes": {} }),
        json!({ "targeting_key": "u", "extra": 1 }),
        json!({ "targeting_key": "u".repeat(513) }),
        json!({ "targeting_key": "u", "attributes": (0..101).map(|i| (format!("a{i}"), json!("v"))).collect::<serde_json::Map<_, _>>() }),
    ] {
        let r = evaluate(&admin, &server, &env_id, "theme", bad.clone()).await;
        assert_eq!(r.status(), 422, "{bad}");
        assert!(
            r.json::<Value>().await.unwrap()["error"].is_string(),
            "{bad}"
        );
    }
    // Over the API body limit.
    let huge = json!({ "targeting_key": "u", "attributes": { "a": "x".repeat(70_000) } });
    assert_eq!(
        evaluate(&admin, &server, &env_id, "theme", huge)
            .await
            .status(),
        413
    );
    // Unknown and invalid keys.
    assert_eq!(
        evaluate(
            &admin,
            &server,
            &env_id,
            "nope",
            json!({ "targeting_key": "u" })
        )
        .await
        .status(),
        404
    );
    assert_eq!(
        evaluate(
            &admin,
            &server,
            &env_id,
            "bad%20key",
            json!({ "targeting_key": "u" })
        )
        .await
        .status(),
        404
    );

    // An SDK key cannot use it, even for its own environment.
    assert_eq!(
        bearer_client(&sdk_key)
            .post(server.url(&format!("/api/environments/{env_id}/flags/theme/evaluate")))
            .json(&json!({ "targeting_key": "u" }))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    // Unauthenticated.
    assert_eq!(
        evaluate(
            &reqwest::Client::new(),
            &server,
            &env_id,
            "theme",
            json!({ "targeting_key": "u" })
        )
        .await
        .status(),
        401
    );
}

#[tokio::test]
async fn playground_follows_project_membership() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_playground_rbac").await;
    let server = Server::spawn(&db, &env.redis_url, &[]).await;
    let (admin, _sdk_key, env_a) = complete_setup(&server).await;
    request(
        &admin,
        "POST",
        &server.url(&format!("/api/environments/{env_a}/flags")),
        Some(json!({ "key": "secret-launch", "is_enabled": true, "rollout_percentage": null, "description": null })),
        200,
    )
    .await;

    let pool = sqlx::PgPool::connect(&db).await.unwrap();
    let project_a: String =
        sqlx::query_scalar("SELECT project_id::text FROM environments WHERE id = $1::uuid")
            .bind(&env_a)
            .fetch_one(&pool)
            .await
            .unwrap();
    pool.close().await;
    let project_b = request(
        &admin,
        "POST",
        &server.url("/api/projects"),
        Some(json!({ "name": "Other" })),
        200,
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let mut ids = Vec::new();
    for (email, role) in [
        ("viewer-a@acme.test", "viewer"),
        ("editor-b@acme.test", "editor"),
        ("outsider@acme.test", "viewer"),
    ] {
        let user = request(
            &admin,
            "POST",
            &server.url("/api/users"),
            Some(
                json!({ "name": email, "email": email, "role": role, "password": ADMIN_PASSWORD }),
            ),
            200,
        )
        .await;
        ids.push(user["id"].as_i64().unwrap());
    }
    for (project, user, role) in [
        (&project_a, ids[0], "viewer"),
        (&project_b, ids[1], "editor"),
    ] {
        request(
            &admin,
            "POST",
            &server.url(&format!("/api/projects/{project}/members")),
            Some(json!({ "user_id": user, "role": role })),
            200,
        )
        .await;
    }
    let viewer_a = login(&server, "viewer-a@acme.test", ADMIN_PASSWORD).await;
    let editor_b = login(&server, "editor-b@acme.test", ADMIN_PASSWORD).await;
    let outsider = login(&server, "outsider@acme.test", ADMIN_PASSWORD).await;
    let body = json!({ "targeting_key": "u" });

    // A viewer of the project may debug it; a read_only token too.
    assert_eq!(
        evaluate(&viewer_a, &server, &env_a, "secret-launch", body.clone())
            .await
            .status(),
        200
    );
    let token = request(
        &viewer_a,
        "POST",
        &server.url("/api/tokens"),
        Some(json!({ "name": "ci", "scope": "read_only" })),
        200,
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let read_only = bearer_client(&token);
    let r = read_only
        .post(server.url(&format!(
            "/api/environments/{env_a}/flags/secret-launch/evaluate"
        )))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // Members of another project and users in no project cannot — and the
    // answer does not reveal whether the flag exists.
    for (who, client) in [("editor of B", &editor_b), ("outsider", &outsider)] {
        let existing = evaluate(client, &server, &env_a, "secret-launch", body.clone())
            .await
            .status();
        let missing = evaluate(client, &server, &env_a, "no-such-flag", body.clone())
            .await
            .status();
        assert!(existing == 403 || existing == 404, "{who}: {existing}");
        assert_eq!(existing, missing, "{who}: existence must not leak");
    }
}
