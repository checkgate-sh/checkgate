#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! Regression tests for the v0.1.31 hardening fixes, against a real server:
//!
//! - configuration limits (segment-expansion amplification, prerequisite
//!   fan-out) reject on create, patch and segment writes;
//! - `/flags/snapshot` reads flags and segments as one snapshot, and fails
//!   loudly on an undecodable flag instead of silently omitting it;
//! - personal access tokens cannot outlive the token that created them, and a
//!   read-only token cannot revoke tokens;
//! - audit-log paging is clamped (a negative or huge `limit` no longer 500s or
//!   dumps the whole history).

mod common;

use common::*;
use serde_json::{Value, json};

struct Fixture {
    server: Server,
    admin: reqwest::Client,
    sdk_key: String,
    env: String,
    db: String,
}

async fn fixture(name: &str) -> Option<Fixture> {
    let env = test_env()?;
    let db = env.fresh_database(name).await;
    let server = Server::spawn(&db, &env.redis_url, &[]).await;
    let (admin, sdk_key, env_id) = complete_setup(&server).await;
    Some(Fixture {
        server,
        admin,
        sdk_key,
        env: env_id,
        db,
    })
}

impl Fixture {
    fn url(&self, path: &str) -> String {
        self.server.url(path)
    }

    fn flags(&self) -> String {
        self.url(&format!("/api/environments/{}/flags", self.env))
    }

    fn segments(&self) -> String {
        self.url(&format!("/api/environments/{}/segments", self.env))
    }

    async fn snapshot(&self) -> (u16, Value) {
        let r = bearer_client(&self.sdk_key)
            .get(self.url("/flags/snapshot"))
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }
}

fn flag(key: &str) -> Value {
    json!({ "key": key, "is_enabled": true, "rollout_percentage": null, "description": null })
}

#[tokio::test]
async fn configuration_limits_reject_amplifying_writes() {
    let Some(f) = fixture("cg_hardening_limits").await else {
        return;
    };
    request(
        &f.admin,
        "POST",
        &f.segments(),
        Some(json!({ "name": "Beta", "key": "beta",
            "rules": [{ "attribute": "plan", "operator": "equals", "values": ["beta"] }] })),
        200,
    )
    .await;

    // Referencing one segment many times copied its rules once per reference.
    let mut dup = flag("dup-segments");
    dup["rules"] = json!([{ "segment_key": "beta" }, { "segment_key": "beta" }]);
    request(&f.admin, "POST", &f.flags(), Some(dup), 422).await;

    let mut own = flag("self-prereq");
    own["prerequisites"] = json!([{ "flag_key": "self-prereq" }]);
    request(&f.admin, "POST", &f.flags(), Some(own), 422).await;

    // An ordinary flag is accepted, then a patch past the limit is not — the
    // check runs on every patch path, not only on create.
    let mut ok = flag("ordinary");
    ok["rules"] = json!([{ "segment_key": "beta" }]);
    ok["prerequisites"] = json!([{ "flag_key": "dup-free" }]);
    request(&f.admin, "POST", &f.flags(), Some(ok), 200).await;
    let too_many: Vec<Value> = (0..51)
        .map(|i| json!({ "flag_key": format!("p{i}") }))
        .collect();
    request(
        &f.admin,
        "PATCH",
        &format!("{}/ordinary", f.flags()),
        Some(json!({ "prerequisites": too_many })),
        422,
    )
    .await;

    let rules: Vec<Value> = (0..501)
        .map(|i| json!({ "attribute": "a", "operator": "equals", "values": [format!("{i}")] }))
        .collect();
    request(
        &f.admin,
        "POST",
        &f.segments(),
        Some(json!({ "name": "Huge", "key": "huge", "rules": rules })),
        422,
    )
    .await;
    request(
        &f.admin,
        "PATCH",
        &format!("{}/beta", f.segments()),
        Some(json!({ "rules": rules })),
        422,
    )
    .await;
}

#[tokio::test]
async fn snapshot_expands_segments_and_fails_on_an_undecodable_flag() {
    let Some(f) = fixture("cg_hardening_snapshot").await else {
        return;
    };
    request(
        &f.admin,
        "POST",
        &f.segments(),
        Some(json!({ "name": "Staff", "key": "staff",
            "rules": [{ "attribute": "email", "operator": "ends_with", "values": ["@acme.test"] }] })),
        200,
    )
    .await;
    let mut targeted = flag("targeted");
    targeted["rules"] = json!([{ "segment_key": "staff" }]);
    request(&f.admin, "POST", &f.flags(), Some(targeted), 200).await;
    request(&f.admin, "POST", &f.flags(), Some(flag("plain")), 200).await;

    let (status, body) = f.snapshot().await;
    assert_eq!(status, 200);
    let flags = body.as_array().unwrap();
    let keys: Vec<&str> = flags.iter().map(|f| f["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["plain", "targeted"], "sorted by key");
    let rule = &flags[1]["rules"][0];
    assert_eq!(rule["attribute"], "email", "segment expanded: {rule}");
    assert_eq!(rule["values"][0], "@acme.test");

    // A row that no longer decodes (e.g. written by a newer server mid-upgrade)
    // used to vanish from the snapshot, which polling SDKs read as a delete.
    let pool = sqlx::PgPool::connect(&f.db).await.unwrap();
    sqlx::query("UPDATE flags SET data = '{\"key\":\"plain\"}'::jsonb WHERE key = 'plain'")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let (status, _) = f.snapshot().await;
    assert_eq!(status, 500, "an undecodable flag fails the snapshot");
}

#[tokio::test]
async fn tokens_cannot_outlive_or_revoke_beyond_their_scope() {
    let Some(f) = fixture("cg_hardening_tokens").await else {
        return;
    };
    let created = request(
        &f.admin,
        "POST",
        &f.url("/api/tokens"),
        Some(json!({ "name": "ci", "scope": "read_only", "expires_in_days": 7 })),
        200,
    )
    .await;
    let read_only = bearer_client(created["token"].as_str().unwrap());
    let other = request(
        &f.admin,
        "POST",
        &f.url("/api/tokens"),
        Some(json!({ "name": "deploy", "scope": "read_write", "expires_in_days": 30 })),
        200,
    )
    .await;

    // A 7-day token minting a permanent one (or a 30-day one) would turn a
    // time-limited credential into a longer-lived one.
    for expires in [Value::Null, json!(30)] {
        let r = read_only
            .post(f.url("/api/tokens"))
            .json(&json!({ "name": "forever", "scope": "read_only", "expires_in_days": expires }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403, "expires_in_days={expires}");
    }
    let r = read_only
        .post(f.url("/api/tokens"))
        .json(&json!({ "name": "short", "scope": "read_only", "expires_in_days": 3 }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "a shorter-lived token is still allowed");

    // Revoking is a write.
    let r = read_only
        .delete(f.url(&format!("/api/tokens/{}", other["id"])))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
}

#[tokio::test]
async fn audit_paging_is_clamped() {
    let Some(f) = fixture("cg_hardening_audit").await else {
        return;
    };
    request(&f.admin, "POST", &f.flags(), Some(flag("audited")), 200).await;
    let audit = f.url(&format!("/api/environments/{}/audit", f.env));
    for query in ["limit=-1", "offset=-5", "limit=100000000"] {
        request(&f.admin, "GET", &format!("{audit}?{query}"), None, 200).await;
    }
    let all = request(
        &f.admin,
        "GET",
        &format!("{audit}?limit=100000000"),
        None,
        200,
    )
    .await;
    assert_eq!(all.as_array().unwrap().len(), 1);
}
