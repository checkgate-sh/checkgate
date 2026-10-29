#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! Flags with the same key in different environments never leak into each
//! other, on any server path an SDK or the dashboard uses: REST reads, the
//! poll snapshot, SSE bootstrap, live SSE deltas (local and Redis-delivered
//! from another instance), deletes, recreates and server restarts.
//!
//! Each SDK-side store is scoped to one environment, exactly as in the SDKs,
//! and evaluated with the shared core so targeting rules, weighted variants
//! and prerequisites are checked end to end.

mod common;

use checkgate_core::evaluator::{Flag, FlagValue, UserContext, evaluate_variant};
use checkgate_core::store::FlagStore;
use common::*;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;

const KEY: &str = "checkout";

type SseEvent = (String, String);

/// Connects to `/stream` with an SDK key and forwards parsed frames.
fn open_stream(base: &str, sdk_key: &str) -> mpsc::UnboundedReceiver<SseEvent> {
    let (tx, rx) = mpsc::unbounded_channel();
    let url = format!("{base}/stream");
    let key = sdk_key.to_string();
    tokio::spawn(async move {
        let resp = reqwest::Client::new()
            .get(&url)
            .bearer_auth(&key)
            .send()
            .await
            .expect("open sse");
        assert_eq!(resp.status(), 200);
        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        let (mut event, mut data) = (String::new(), String::new());
        while let Some(Ok(chunk)) = stream.next().await {
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(nl) = buf.find('\n') {
                let line = buf[..nl].trim_end_matches('\r').to_string();
                buf.drain(..=nl);
                if line.is_empty() {
                    if !event.is_empty() || !data.is_empty() {
                        let _ = tx.send((std::mem::take(&mut event), std::mem::take(&mut data)));
                    }
                } else if let Some(v) = line.strip_prefix("event:") {
                    event = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("data:") {
                    data.push_str(v.trim());
                }
            }
        }
    });
    rx
}

/// An SDK's view of one environment: its stream and its local store.
struct Sdk {
    env_id: String,
    rx: mpsc::UnboundedReceiver<SseEvent>,
    store: FlagStore,
}

impl Sdk {
    async fn connect(base: &str, sdk_key: &str, env_id: &str) -> Self {
        let mut sdk = Sdk {
            env_id: env_id.to_string(),
            rx: open_stream(base, sdk_key),
            store: FlagStore::new(),
        };
        let connected = sdk.next_event().await;
        assert_eq!(connected.0, "connected");
        assert_eq!(
            serde_json::from_str::<Value>(&connected.1).unwrap()["environment_id"],
            env_id
        );
        // Apply the bootstrap replay up to `ready`.
        loop {
            let (event, data) = sdk.next_event().await;
            if event == "ready" {
                break;
            }
            sdk.apply(&event, &data);
        }
        sdk
    }

    async fn next_event(&mut self) -> SseEvent {
        tokio::time::timeout(Duration::from_secs(10), self.rx.recv())
            .await
            .expect("SSE event in time")
            .expect("SSE stream open")
    }

    /// Applies an `update` frame, asserting it belongs to this environment.
    /// Returns the flag key it touched.
    fn apply(&self, event: &str, data: &str) -> Option<String> {
        if event != "update" {
            return None;
        }
        let msg: Value = serde_json::from_str(data).unwrap();
        assert_eq!(
            msg["env_id"], self.env_id,
            "an SDK only ever receives its own environment's events: {data}"
        );
        match msg["type"].as_str() {
            Some("UPSERT") => {
                let flag: Flag = serde_json::from_value(msg["flag"].clone()).unwrap();
                let key = flag.key.clone();
                self.store.upsert_flag(flag);
                Some(key)
            }
            Some("DELETE") => {
                let key = msg["key"].as_str().unwrap().to_string();
                self.store.delete_flag(&key);
                Some(key)
            }
            _ => None,
        }
    }

    /// Applies events until one touches `key`; returns every key touched.
    async fn until(&mut self, key: &str) -> Vec<String> {
        let mut touched = Vec::new();
        loop {
            let (event, data) = self.next_event().await;
            if let Some(k) = self.apply(&event, &data) {
                touched.push(k.clone());
                if k == key {
                    return touched;
                }
            }
        }
    }

    fn eval(&self, email: &str) -> Option<(bool, FlagValue)> {
        let flag = self.store.get_flag(KEY)?;
        let ctx = UserContext {
            key: "user-1".into(),
            attributes: HashMap::from([("email".to_string(), email.to_string())]),
        };
        let r = evaluate_variant(&flag, &ctx, &self.store);
        Some((r.enabled, r.value))
    }
}

fn s(v: &str) -> FlagValue {
    FlagValue::Str(v.into())
}

struct Fixture {
    admin: reqwest::Client,
    keys: [String; 3],
    envs: [String; 3],
}

impl Fixture {
    fn flags_url(&self, server: &Server, env: usize) -> String {
        server.url(&format!("/api/environments/{}/flags", self.envs[env]))
    }

    /// Bumps a per-environment marker flag. Events on one instance are
    /// delivered in publish order, so once an SDK sees its marker it has
    /// seen every earlier event meant for it.
    async fn marker(&self, server: &Server, env: usize, n: u32) {
        request(
            &self.admin,
            "POST",
            &self.flags_url(server, env),
            Some(json!({ "key": "marker", "is_enabled": true, "flag_type": "boolean", "rules": [], "description": n.to_string() })),
            200,
        )
        .await;
    }
}

/// Environments: 0 = Production (seeded), 1 = a second environment in the same
/// project, 2 = an environment in another project. All three define `checkout`
/// and `gate` with different values.
async fn fixture(server: &Server, db: &str) -> Fixture {
    let (admin, prod_key, prod_env) = complete_setup(server).await;
    let pool = sqlx::PgPool::connect(db).await.unwrap();
    let project: String =
        sqlx::query_scalar("SELECT project_id::text FROM environments WHERE id = $1::uuid")
            .bind(&prod_env)
            .fetch_one(&pool)
            .await
            .unwrap();
    pool.close().await;
    let other_project = request(
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

    let mut envs = vec![prod_env];
    let mut keys = vec![prod_key];
    for (project, slug) in [(&project, "iso-two"), (&other_project, "iso-three")] {
        let env = request(
            &admin,
            "POST",
            &server.url(&format!("/api/projects/{project}/environments")),
            Some(json!({ "name": slug, "slug": slug })),
            200,
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_string();
        let key = request(
            &admin,
            "POST",
            &server.url(&format!("/api/projects/{project}/keys")),
            Some(json!({ "name": slug, "environment_id": env })),
            200,
        )
        .await["key"]
            .as_str()
            .unwrap()
            .to_string();
        envs.push(env);
        keys.push(key);
    }
    let f = Fixture {
        admin,
        keys: keys.try_into().unwrap(),
        envs: envs.try_into().unwrap(),
    };

    let gate = |enabled: bool| json!({ "key": "gate", "is_enabled": enabled, "flag_type": "boolean", "rules": [] });
    let checkout = [
        // 0: targeting rule + prerequisite on an enabled gate.
        json!({
            "key": KEY, "is_enabled": true, "flag_type": "string",
            "default_value": "alpha", "disabled_value": "alpha-off",
            "rules": [{ "attribute": "email", "operator": "ends_with", "values": ["@vip.test"], "variant": "alpha-vip" }],
            "prerequisites": [{ "flag_key": "gate" }],
        }),
        // 1: weighted variant, same prerequisite.
        json!({
            "key": KEY, "is_enabled": true, "flag_type": "string",
            "default_value": "beta", "disabled_value": "beta-off",
            "variants": [{ "weight": 1, "value": "beta-weighted" }],
            "rules": [], "prerequisites": [{ "flag_key": "gate" }],
        }),
        // 2: same prerequisite, but this environment's `gate` is off — the
        // prerequisite must resolve against this environment's own `gate`.
        json!({
            "key": KEY, "is_enabled": true, "flag_type": "string",
            "default_value": "gamma", "disabled_value": "gamma-off",
            "rules": [], "prerequisites": [{ "flag_key": "gate" }],
        }),
    ];
    for (i, body) in checkout.into_iter().enumerate() {
        request(
            &f.admin,
            "POST",
            &f.flags_url(server, i),
            Some(gate(i != 2)),
            200,
        )
        .await;
        request(&f.admin, "POST", &f.flags_url(server, i), Some(body), 200).await;
    }
    f
}

/// What each environment's `checkout` evaluates to for a regular and a VIP user.
fn expected_initial() -> [Option<[(bool, FlagValue); 2]>; 3] {
    [
        Some([(true, s("alpha")), (true, s("alpha-vip"))]),
        Some([(true, s("beta-weighted")), (true, s("beta-weighted"))]),
        Some([(false, s("gamma-off")), (false, s("gamma-off"))]),
    ]
}

fn assert_evals(sdks: &[Sdk], expected: &[Option<[(bool, FlagValue); 2]>; 3], when: &str) {
    for (i, sdk) in sdks.iter().enumerate() {
        let got = sdk
            .eval("someone@acme.test")
            .map(|regular| [regular, sdk.eval("boss@vip.test").unwrap()]);
        assert_eq!(got, expected[i], "environment {i} {when}");
    }
}

async fn snapshot(server: &Server, key: &str) -> (String, Vec<Value>) {
    let r = bearer_client(key)
        .get(server.url("/flags/snapshot"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let env = r.headers()["x-checkgate-environment-id"]
        .to_str()
        .unwrap()
        .to_string();
    (env, r.json().await.unwrap())
}

fn snapshot_default(flags: &[Value]) -> Option<Value> {
    flags
        .iter()
        .find(|f| f["key"] == KEY)
        .map(|f| f["default_value"].clone())
}

#[tokio::test]
async fn identical_flag_keys_stay_isolated_per_environment() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_env_isolation").await;
    let server = Server::spawn(&db, &env.redis_url, &[]).await;
    let f = fixture(&server, &db).await;

    // REST reads and poll snapshots return each environment's own row.
    for (i, want) in ["alpha", "beta", "gamma"].into_iter().enumerate() {
        let flag = request(
            &f.admin,
            "GET",
            &format!("{}/{KEY}", f.flags_url(&server, i)),
            None,
            200,
        )
        .await;
        assert_eq!(flag["default_value"], want, "REST env {i}");
        let (header_env, flags) = snapshot(&server, &f.keys[i]).await;
        assert_eq!(header_env, f.envs[i]);
        assert_eq!(
            snapshot_default(&flags),
            Some(json!(want)),
            "snapshot env {i}"
        );
    }

    // SSE bootstrap: each SDK store holds only its environment's flags.
    let mut sdks = Vec::new();
    for i in 0..3 {
        sdks.push(Sdk::connect(&server.base, &f.keys[i], &f.envs[i]).await);
    }
    assert_evals(&sdks, &expected_initial(), "after bootstrap");

    // Updating environment 0 reaches only environment 0.
    request(
        &f.admin,
        "PATCH",
        &format!("{}/{KEY}", f.flags_url(&server, 0)),
        Some(json!({ "default_value": "alpha-2" })),
        200,
    )
    .await;
    assert_eq!(sdks[0].until(KEY).await, vec![KEY]);
    for (i, sdk) in sdks.iter_mut().enumerate().skip(1) {
        f.marker(&server, i, 1).await;
        assert_eq!(
            sdk.until("marker").await,
            vec!["marker"],
            "env {i} saw no foreign update"
        );
    }
    let mut expected = expected_initial();
    expected[0] = Some([(true, s("alpha-2")), (true, s("alpha-vip"))]);
    assert_evals(&sdks, &expected, "after updating env 0");

    // Deleting environment 1's flag leaves the others intact.
    request(
        &f.admin,
        "DELETE",
        &format!("{}/{KEY}", f.flags_url(&server, 1)),
        None,
        204,
    )
    .await;
    assert_eq!(sdks[1].until(KEY).await, vec![KEY]);
    for i in [0, 2] {
        f.marker(&server, i, 2).await;
        assert_eq!(
            sdks[i].until("marker").await,
            vec!["marker"],
            "env {i} saw no foreign delete"
        );
    }
    expected[1] = None;
    assert_evals(&sdks, &expected, "after deleting env 1");
    assert_eq!(
        snapshot_default(&snapshot(&server, &f.keys[1]).await.1),
        None
    );
    assert_eq!(
        snapshot_default(&snapshot(&server, &f.keys[0]).await.1),
        Some(json!("alpha-2"))
    );
    request(
        &f.admin,
        "GET",
        &format!("{}/{KEY}", f.flags_url(&server, 1)),
        None,
        404,
    )
    .await;
    request(
        &f.admin,
        "GET",
        &format!("{}/{KEY}", f.flags_url(&server, 2)),
        None,
        200,
    )
    .await;

    // Recreating it affects only environment 1 again.
    request(
        &f.admin,
        "POST",
        &f.flags_url(&server, 1),
        Some(json!({ "key": KEY, "is_enabled": true, "flag_type": "string", "default_value": "beta-2", "rules": [] })),
        200,
    )
    .await;
    assert_eq!(sdks[1].until(KEY).await, vec![KEY]);
    for i in [0, 2] {
        f.marker(&server, i, 3).await;
        assert_eq!(
            sdks[i].until("marker").await,
            vec!["marker"],
            "env {i} saw no foreign recreate"
        );
    }
    expected[1] = Some([(true, s("beta-2")), (true, s("beta-2"))]);
    assert_evals(&sdks, &expected, "after recreating env 1");
    drop(sdks);

    // A restarted server bootstraps every environment from its own rows.
    let server = server.restart(&db, &env.redis_url, &[]).await;
    let mut sdks = Vec::new();
    for i in 0..3 {
        sdks.push(Sdk::connect(&server.base, &f.keys[i], &f.envs[i]).await);
    }
    assert_evals(&sdks, &expected, "after restart");

    // Redis-delivered updates: a write on one instance reaches the matching
    // environment's SDKs on another instance, and only those.
    let second = Server::spawn(&db, &env.redis_url, &[]).await;
    let mut remote = Vec::new();
    for i in 0..3 {
        remote.push(Sdk::connect(&second.base, &f.keys[i], &f.envs[i]).await);
    }
    assert_evals(&remote, &expected, "second instance bootstrap");
    request(
        &f.admin,
        "PATCH",
        &format!("{}/gate", f.flags_url(&server, 2)),
        Some(json!({ "is_enabled": true })),
        200,
    )
    .await;
    assert_eq!(remote[2].until("gate").await, vec!["gate"]);
    assert_eq!(sdks[2].until("gate").await, vec!["gate"]);
    for i in 0..2 {
        f.marker(&server, i, 4).await;
        assert_eq!(
            remote[i].until("marker").await,
            vec!["marker"],
            "remote env {i}"
        );
        assert_eq!(
            sdks[i].until("marker").await,
            vec!["marker"],
            "local env {i}"
        );
    }
    expected[2] = Some([(true, s("gamma")), (true, s("gamma"))]);
    assert_evals(&remote, &expected, "second instance after remote update");
    assert_evals(&sdks, &expected, "first instance after update");
}
