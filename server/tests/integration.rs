#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! End-to-end server integration tests.
//!
//! These spawn the **real** compiled server binary against a real Postgres and
//! Redis, then drive the HTTP API exactly as a client would — exercising
//! migrations, auth, RBAC, flag/segment CRUD, analytics (impressions, exposure),
//! A/B events + experiments, and the change-request approval gate.
//!
//! Requires two environment variables (the suite skips cleanly if they're unset,
//! so `cargo test --workspace` stays green where no database is available):
//!   CHECKGATE_TEST_DATABASE_URL  e.g. postgres://postgres@localhost:55432/checkgate_test
//!   CHECKGATE_TEST_REDIS_URL     e.g. redis://localhost:56379
//!
//! The target database's `public` schema is dropped and recreated at the start
//! of the run, so each run begins from a clean, freshly-migrated state.

use serde_json::{Value, json};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Reads the two env vars the suite needs, or signals that it should be skipped.
///
/// Skipping is the right default: `cargo test --workspace` must stay green on a
/// machine with no database, and the generic workspace CI job runs without one
/// too. But the job that exists *specifically* to run this suite must never skip
/// quietly and still report `ok` — so that job sets
/// `CHECKGATE_REQUIRE_INTEGRATION=1`, and a missing database is then a hard
/// failure pointing at the workflow rather than a silent pass.
fn test_env() -> Option<(String, String)> {
    match (
        std::env::var("CHECKGATE_TEST_DATABASE_URL"),
        std::env::var("CHECKGATE_TEST_REDIS_URL"),
    ) {
        (Ok(db), Ok(redis)) => Some((db, redis)),
        _ => {
            assert!(
                std::env::var("CHECKGATE_REQUIRE_INTEGRATION").is_err(),
                "CHECKGATE_REQUIRE_INTEGRATION is set but CHECKGATE_TEST_DATABASE_URL / \
                 CHECKGATE_TEST_REDIS_URL are not — this suite would skip silently and still \
                 report success. Check the `env:` block of the server-integration job."
            );
            eprintln!(
                "SKIP: set CHECKGATE_TEST_DATABASE_URL and CHECKGATE_TEST_REDIS_URL to run server integration tests"
            );
            None
        }
    }
}

/// Kills the spawned server when the test ends (including on panic).
struct ServerGuard(Child);
impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn reset_schema(db_url: &str) {
    let pool = sqlx::PgPool::connect(db_url)
        .await
        .expect("connect to test database");
    // Two separate statements — the extended (prepared) protocol sqlx uses
    // rejects multiple commands in one query.
    sqlx::query("DROP SCHEMA public CASCADE")
        .execute(&pool)
        .await
        .expect("drop public schema");
    sqlx::query("CREATE SCHEMA public")
        .execute(&pool)
        .await
        .expect("create public schema");
    pool.close().await;
}

async fn wait_healthy(base: &str) {
    let client = reqwest::Client::new();
    for _ in 0..120 {
        if let Ok(resp) = client.get(format!("{base}/health")).send().await
            && resp.status().is_success()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("server did not become healthy at {base}");
}

/// A tiny helper bundling the base URL + a cookie-storing client (session) and a
/// cookieless client (Bearer / anonymous).
struct Ctx {
    base: String,
    sess: reqwest::Client,
    anon: reqwest::Client,
}

impl Ctx {
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }
}

#[tokio::test]
async fn full_api_flow() {
    let Some((db_url, redis_url)) = test_env() else {
        return;
    };

    reset_schema(&db_url).await;

    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let public_dir = std::env::temp_dir().join(format!("checkgate-test-public-{port}"));
    std::fs::create_dir_all(&public_dir).unwrap();

    let child = Command::new(env!("CARGO_BIN_EXE_server"))
        .env("DATABASE_URL", &db_url)
        .env("REDIS_URL", &redis_url)
        .env("PORT", port.to_string())
        .env(
            "SESSION_SECRET",
            "test-secret-0123456789-0123456789-0123456789-0123456789-0123456789",
        )
        .env("COOKIE_SECURE", "false")
        .env("PUBLIC_DIR", public_dir.to_str().unwrap())
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn server binary");
    let _guard = ServerGuard(child);

    wait_healthy(&base).await;

    let ctx = Ctx {
        base,
        sess: reqwest::Client::builder()
            .cookie_store(true)
            .build()
            .unwrap(),
        anon: reqwest::Client::new(),
    };

    // Run the ordered scenario. Each helper asserts as it goes.
    let (sdk_key, prod_env) = setup_and_keys(&ctx).await;
    auth_failures(&ctx, &prod_env).await;
    flag_crud(&ctx, &sdk_key, &prod_env).await;
    segment_crud(&ctx, &sdk_key, &prod_env).await;
    analytics_and_experiments(&ctx, &sdk_key, &prod_env).await;
    snapshot(&ctx, &sdk_key).await;
    rbac_and_change_requests(&ctx, &prod_env).await;
    // Runs last on purpose: `rbac_and_change_requests` leaves Production in
    // approval-required mode, so a PATCH here queues a change request and
    // exercises the change_request.* notifications alongside the flag ones.
    chat_integrations(&ctx, &prod_env).await;
    security_regressions(&ctx, &db_url, &redis_url, &sdk_key, &prod_env).await;

    eprintln!("integration: full_api_flow passed");
}

// --- Setup + credentials ---------------------------------------------------

async fn setup_and_keys(ctx: &Ctx) -> (String, String) {
    // Before setup, the seeded Production SDK key + environment id are readable.
    let r = ctx
        .anon
        .get(ctx.url("/api/setup/key"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "GET /api/setup/key before setup");
    let key_body: Value = r.json().await.unwrap();
    let sdk_key = key_body["key"].as_str().unwrap().to_string();
    let prod_env = key_body["environment_id"].as_str().unwrap().to_string();
    assert!(sdk_key.starts_with("sk_"), "sdk key looks valid: {sdk_key}");

    // Complete first-run setup (creates the admin + session cookie).
    let r = ctx
        .sess
        .post(ctx.url("/api/setup/complete"))
        .header("X-Checkgate-Request", "true")
        .json(&json!({
            "workspace_name": "Acme",
            "project_name": "Web",
            "name": "Admin",
            "email": "admin@acme.test",
            "password": "supersecret1",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "setup/complete");
    let me: Value = r.json().await.unwrap();
    assert_eq!(me["role"], "admin");
    assert_eq!(me["is_setup_complete"], true);

    // Session cookie now works.
    let r = ctx.sess.get(ctx.url("/api/auth/me")).send().await.unwrap();
    assert_eq!(r.status(), 200, "auth/me with session");
    assert_eq!(r.json::<Value>().await.unwrap()["email"], "admin@acme.test");

    // Setup is one-time.
    let r = ctx
        .sess
        .post(ctx.url("/api/setup/complete"))
        .header("X-Checkgate-Request", "true")
        .json(&json!({ "workspace_name": "x", "name": "x", "email": "x@x.t", "password": "supersecret1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404, "second setup rejected");

    (sdk_key, prod_env)
}

// --- Auth failures ---------------------------------------------------------

async fn auth_failures(ctx: &Ctx, env: &str) {
    // No credentials → 401.
    let r = ctx
        .anon
        .get(ctx.url(&format!("/api/environments/{env}/flags")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "unauthenticated flag list");

    // Bad bearer → 401.
    let r = ctx
        .anon
        .get(ctx.url(&format!("/api/environments/{env}/flags")))
        .bearer_auth("sk_totally_invalid")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "invalid bearer flag list");
}

// --- Flag CRUD (via SDK key, which is admin-equivalent + CSRF-exempt) -------

async fn flag_crud(ctx: &Ctx, sdk_key: &str, env: &str) {
    let flags = format!("/api/environments/{env}/flags");

    let r = ctx
        .anon
        .post(ctx.url(&flags))
        .bearer_auth(sdk_key)
        .json(&json!({
            "key": "new-homepage",
            "is_enabled": true,
            "rollout_percentage": 50,
            "description": "rollout",
            "flag_type": "boolean",
            "rules": []
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "create flag");
    assert_eq!(r.json::<Value>().await.unwrap()["key"], "new-homepage");

    let r = ctx
        .anon
        .get(ctx.url(&format!("{flags}/new-homepage")))
        .bearer_auth(sdk_key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "get flag");
    assert_eq!(r.json::<Value>().await.unwrap()["rollout_percentage"], 50);

    let list: Value = ctx
        .anon
        .get(ctx.url(&flags))
        .bearer_auth(sdk_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .any(|f| f["key"] == "new-homepage"),
        "list contains created flag"
    );

    // PATCH applies immediately (Production has no approval requirement yet).
    let r = ctx
        .anon
        .patch(ctx.url(&format!("{flags}/new-homepage")))
        .bearer_auth(sdk_key)
        .json(&json!({ "is_enabled": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "patch flag applied");
    assert_eq!(r.json::<Value>().await.unwrap()["is_enabled"], false);

    let r = ctx
        .anon
        .delete(ctx.url(&format!("{flags}/new-homepage")))
        .bearer_auth(sdk_key)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "delete flag");

    let r = ctx
        .anon
        .get(ctx.url(&format!("{flags}/new-homepage")))
        .bearer_auth(sdk_key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404, "deleted flag is gone");
}

// --- Segment CRUD ----------------------------------------------------------

async fn segment_crud(ctx: &Ctx, sdk_key: &str, env: &str) {
    let segs = format!("/api/environments/{env}/segments");
    let r = ctx
        .anon
        .post(ctx.url(&segs))
        .bearer_auth(sdk_key)
        .json(&json!({
            "key": "internal",
            "name": "Internal",
            "rules": [{ "attribute": "email", "operator": "ends_with", "values": ["@acme.test"] }]
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "create segment: {}", r.status());

    let r = ctx
        .anon
        .get(ctx.url(&format!("{segs}/internal")))
        .bearer_auth(sdk_key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "get segment");
    assert_eq!(r.json::<Value>().await.unwrap()["name"], "Internal");

    let r = ctx
        .anon
        .delete(ctx.url(&format!("{segs}/internal")))
        .bearer_auth(sdk_key)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "delete segment");
}

// --- Impressions, exposure, events, experiments ----------------------------

async fn analytics_and_experiments(ctx: &Ctx, sdk_key: &str, env: &str) {
    let base = format!("/api/environments/{env}");

    // A string flag with two variants to experiment on.
    let r = ctx
        .anon
        .post(ctx.url(&format!("{base}/flags")))
        .bearer_auth(sdk_key)
        .json(&json!({
            "key": "exp-flag",
            "is_enabled": true,
            "rollout_percentage": null,
            "description": "experiment",
            "flag_type": "string",
            "default_value": "control",
            "variants": [
                { "weight": 50, "value": "control" },
                { "weight": 50, "value": "treatment" }
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "create experiment flag");

    // Ingest impressions: u1/u2 on control, u3/u4 on treatment.
    let impressions = json!([
        { "flag_key": "exp-flag", "user_id": "u1", "value": "control" },
        { "flag_key": "exp-flag", "user_id": "u2", "value": "control" },
        { "flag_key": "exp-flag", "user_id": "u3", "value": "treatment" },
        { "flag_key": "exp-flag", "user_id": "u4", "value": "treatment" },
    ]);
    let r = ctx
        .anon
        .post(ctx.url(&format!("{base}/impressions")))
        .bearer_auth(sdk_key)
        .json(&impressions)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204, "ingest impressions");

    // Stats + exposure reflect the ingested data.
    let stats: Value = ctx
        .anon
        .get(ctx.url(&format!("{base}/impressions/stats")))
        .bearer_auth(sdk_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        stats
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["flag_key"] == "exp-flag" && s["total"] == 4),
        "impression stats: {stats}"
    );

    let exposure: Value = ctx
        .anon
        .get(ctx.url(&format!("{base}/impressions/exposure?flag_key=exp-flag")))
        .bearer_auth(sdk_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        exposure["total_impressions"], 4,
        "exposure totals: {exposure}"
    );
    assert_eq!(exposure["variants"].as_array().unwrap().len(), 2);

    // Goal events: u1 and u3 convert.
    let events = json!([
        { "event_key": "checkout", "user_id": "u1" },
        { "event_key": "checkout", "user_id": "u3" },
    ]);
    let r = ctx
        .anon
        .post(ctx.url(&format!("{base}/events")))
        .bearer_auth(sdk_key)
        .json(&events)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204, "ingest events");

    let keys: Value = ctx
        .anon
        .get(ctx.url(&format!("{base}/events/keys")))
        .bearer_auth(sdk_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        keys.as_array()
            .unwrap()
            .iter()
            .any(|k| k["event_key"] == "checkout"),
        "event keys: {keys}"
    );

    // Create the experiment and read its results.
    let r = ctx
        .anon
        .post(ctx.url(&format!("{base}/experiments")))
        .bearer_auth(sdk_key)
        .json(&json!({
            "key": "exp1",
            "name": "Checkout experiment",
            "flag_key": "exp-flag",
            "goal_event_key": "checkout"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "create experiment");

    let results: Value = ctx
        .anon
        .get(ctx.url(&format!("{base}/experiments/exp1/results")))
        .bearer_auth(sdk_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // 4 users exposed, 2 converted (one per variant), split across 2 variants.
    assert_eq!(results["total_exposed"], 4, "experiment results: {results}");
    assert_eq!(
        results["total_converted"], 2,
        "experiment results: {results}"
    );
    assert_eq!(results["variants"].as_array().unwrap().len(), 2);
}

// --- Poll-fallback snapshot ------------------------------------------------

async fn snapshot(ctx: &Ctx, sdk_key: &str) {
    let r = ctx
        .anon
        .get(ctx.url("/flags/snapshot"))
        .bearer_auth(sdk_key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "flags snapshot");
    assert!(
        r.json::<Value>().await.unwrap().is_array(),
        "snapshot is an array"
    );
}

// --- RBAC + change-request approval gate -----------------------------------

async fn rbac_and_change_requests(ctx: &Ctx, env: &str) {
    // Admin creates a viewer user.
    let r = ctx
        .sess
        .post(ctx.url("/api/users"))
        .header("X-Checkgate-Request", "true")
        .json(&json!({
            "name": "Val Viewer",
            "email": "viewer@acme.test",
            "role": "viewer",
            "password": "supersecret1"
        }))
        .send()
        .await
        .unwrap();
    assert!(
        r.status().is_success(),
        "admin creates viewer: {}",
        r.status()
    );

    // Viewer logs in (own cookie jar) and is forbidden from writing flags.
    let viewer = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    let r = viewer
        .post(ctx.url("/api/auth/login"))
        .header("X-Checkgate-Request", "true")
        .json(&json!({ "email": "viewer@acme.test", "password": "supersecret1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "viewer login");

    let r = viewer
        .post(ctx.url(&format!("/api/environments/{env}/flags")))
        .header("X-Checkgate-Request", "true")
        .json(&json!({ "key": "sneaky", "is_enabled": true, "flag_type": "boolean", "rules": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403, "viewer cannot create flags");

    // Enable approval on Production, then an admin PATCH is queued (202), not applied.
    let projects: Value = ctx
        .sess
        .get(ctx.url("/api/projects"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let project_id = projects[0]["id"].as_str().unwrap().to_string();

    // Seed a flag to modify under approval.
    ctx.sess
        .post(ctx.url(&format!("/api/environments/{env}/flags")))
        .header("X-Checkgate-Request", "true")
        .json(&json!({ "key": "gated", "is_enabled": true, "flag_type": "boolean", "rules": [] }))
        .send()
        .await
        .unwrap();

    let r = ctx
        .sess
        .post(ctx.url(&format!(
            "/api/projects/{project_id}/environments/{env}/require-approval"
        )))
        .header("X-Checkgate-Request", "true")
        .json(&json!({ "require_approval": true }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "enable approval: {}", r.status());

    let r = ctx
        .sess
        .patch(ctx.url(&format!("/api/environments/{env}/flags/gated")))
        .header("X-Checkgate-Request", "true")
        .json(&json!({ "is_enabled": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 202, "patch under approval is queued");

    let crs: Value = ctx
        .sess
        .get(ctx.url(&format!("/api/environments/{env}/change-requests")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !crs.as_array().unwrap().is_empty(),
        "a pending change request exists: {crs}"
    );
}

// --- Chat integrations (Slack / Teams) -------------------------------------

/// A stand-in for a Slack/Teams incoming webhook: accepts POSTs, records the
/// JSON body, and always answers 200. Lets the test assert on the exact message
/// the server produced, over a real socket, rather than trusting the formatter
/// in isolation.
struct MockChat {
    url: String,
    received: Arc<Mutex<Vec<Value>>>,
}

impl MockChat {
    /// Waits until at least `n` messages have arrived, or fails after ~5s.
    async fn wait_for(&self, n: usize, what: &str) -> Vec<Value> {
        for _ in 0..50 {
            {
                let got = self.received.lock().unwrap();
                if got.len() >= n {
                    return got.clone();
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let got = self.received.lock().unwrap();
        panic!(
            "timed out waiting for {n} message(s) [{what}]; got {}: {:?}",
            got.len(),
            *got
        );
    }

    fn count(&self) -> usize {
        self.received.lock().unwrap().len()
    }
}

/// Splits a raw HTTP request into (header_len, content_length).
fn parse_head(buf: &[u8]) -> Option<(usize, usize)> {
    let pos = buf.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
    let len = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    Some((pos, len))
}

async fn start_mock_chat() -> MockChat {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let received: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = received.clone();

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                continue;
            };
            let sink = sink.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    match socket.read(&mut tmp).await {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if let Some((head_len, body_len)) = parse_head(&buf)
                                && buf.len() >= head_len + body_len
                            {
                                if let Ok(v) = serde_json::from_slice::<Value>(
                                    &buf[head_len..head_len + body_len],
                                ) {
                                    sink.lock().unwrap().push(v);
                                }
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await;
            });
        }
    });

    MockChat {
        url: format!("http://127.0.0.1:{port}/hook"),
        received,
    }
}

/// The display name the API reports for `env_id` — the notification is expected
/// to carry exactly this, which is what proves the delivery path resolves the
/// environment rather than echoing an id.
async fn env_name(ctx: &Ctx, env_id: &str) -> String {
    let projects: Value = ctx
        .sess
        .get(ctx.url("/api/projects"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let project_id = projects[0]["id"].as_str().unwrap().to_string();

    let envs: Value = ctx
        .sess
        .get(ctx.url(&format!("/api/projects/{project_id}/environments")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    envs.as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == env_id)
        .unwrap_or_else(|| panic!("environment {env_id} not found in {envs}"))["name"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn chat_integrations(ctx: &Ctx, env: &str) {
    let env_display = env_name(ctx, env).await;

    // --- validation ---------------------------------------------------------

    let bad_kind = ctx
        .sess
        .post(ctx.url(&format!("/api/environments/{env}/integrations")))
        .header("X-Checkgate-Request", "true")
        .json(&json!({ "kind": "irc", "name": "x", "webhook_url": "https://example.com/h" }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_kind.status(), 422, "unknown provider rejected");

    let plaintext = ctx
        .sess
        .post(ctx.url(&format!("/api/environments/{env}/integrations")))
        .header("X-Checkgate-Request", "true")
        .json(&json!({ "kind": "slack", "name": "x", "webhook_url": "http://evil.example.com/h" }))
        .send()
        .await
        .unwrap();
    assert_eq!(plaintext.status(), 422, "non-loopback plaintext rejected");

    let bad_event = ctx
        .sess
        .post(ctx.url(&format!("/api/environments/{env}/integrations")))
        .header("X-Checkgate-Request", "true")
        .json(&json!({
            "kind": "slack",
            "name": "x",
            "webhook_url": "https://hooks.slack.com/h",
            "events": ["flag.exploded"],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_event.status(), 422, "unknown event name rejected");

    // --- create against a live mock endpoint --------------------------------

    let slack = start_mock_chat().await;
    let teams = start_mock_chat().await;
    // Subscribes to a single event, to prove the filter actually filters.
    let filtered = start_mock_chat().await;

    let r = ctx
        .sess
        .post(ctx.url(&format!("/api/environments/{env}/integrations")))
        .header("X-Checkgate-Request", "true")
        .json(&json!({ "kind": "slack", "name": "Eng channel", "webhook_url": slack.url }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "create slack integration");
    let slack_row: Value = r.json().await.unwrap();
    let slack_id = slack_row["id"].as_str().unwrap().to_string();

    // The webhook URL is a channel-posting credential — it must not come back.
    assert!(
        slack_row.get("webhook_url").is_none(),
        "raw webhook_url must never be returned: {slack_row}"
    );
    let preview = slack_row["webhook_url_preview"].as_str().unwrap();
    assert!(preview.starts_with('…'), "preview is elided: {preview}");
    assert!(
        !preview.contains("127.0.0.1"),
        "preview leaks host: {preview}"
    );

    ctx.sess
        .post(ctx.url(&format!("/api/environments/{env}/integrations")))
        .header("X-Checkgate-Request", "true")
        .json(&json!({ "kind": "teams", "name": "Ops channel", "webhook_url": teams.url }))
        .send()
        .await
        .unwrap();

    ctx.sess
        .post(ctx.url(&format!("/api/environments/{env}/integrations")))
        .header("X-Checkgate-Request", "true")
        .json(&json!({
            "kind": "slack",
            "name": "Deletes only",
            "webhook_url": filtered.url,
            "events": ["flag.deleted"],
        }))
        .send()
        .await
        .unwrap();

    // --- flag.created reaches both providers --------------------------------

    let r = ctx
        .sess
        .post(ctx.url(&format!("/api/environments/{env}/flags")))
        .header("X-Checkgate-Request", "true")
        .json(&json!({
            "key": "chat_notify",
            "is_enabled": true,
            "flag_type": "boolean",
            "rollout_percentage": 25,
            "rules": [],
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "create flag: {}", r.status());

    let slack_msgs = slack.wait_for(1, "slack flag.created").await;
    let msg = &slack_msgs[0];
    // Block Kit shape — a fallback line plus a coloured attachment with blocks.
    assert!(
        msg["text"].as_str().unwrap().contains("Flag created"),
        "slack fallback text: {msg}"
    );
    assert!(msg["text"].as_str().unwrap().contains("chat_notify"));
    assert_eq!(msg["attachments"][0]["blocks"][0]["type"], "header");
    let rendered = msg.to_string();
    assert!(
        rendered.contains(&env_display),
        "names the env ({env_display}): {rendered}"
    );
    assert!(rendered.contains("25%"), "carries rollout: {rendered}");

    let teams_msgs = teams.wait_for(1, "teams flag.created").await;
    let card = &teams_msgs[0];
    assert_eq!(card["@type"], "MessageCard", "teams card shape: {card}");
    let facts = card["sections"][0]["facts"].as_array().unwrap();
    assert!(
        facts
            .iter()
            .any(|f| f["name"] == "Flag" && f["value"] == "chat_notify"),
        "teams facts name the flag: {card}"
    );

    // The delete-only integration must have stayed silent.
    assert_eq!(
        filtered.count(),
        0,
        "event filter must suppress unsubscribed events"
    );

    // --- change_request.opened (Production requires approval by now) ---------

    let r = ctx
        .sess
        .patch(ctx.url(&format!("/api/environments/{env}/flags/chat_notify")))
        .header("X-Checkgate-Request", "true")
        .json(&json!({ "is_enabled": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 202, "patch is queued for approval");

    let slack_msgs = slack.wait_for(2, "slack change_request.opened").await;
    let cr_msg = slack_msgs
        .iter()
        .find(|m| {
            m["text"]
                .as_str()
                .unwrap_or_default()
                .contains("Change request opened")
        })
        .unwrap_or_else(|| panic!("no change-request message in {slack_msgs:?}"));
    assert!(
        cr_msg.to_string().contains("chat_notify"),
        "change-request message names the flag: {cr_msg}"
    );

    // --- delivery log -------------------------------------------------------

    let deliveries: Value = ctx
        .sess
        .get(ctx.url(&format!(
            "/api/environments/{env}/integrations/{slack_id}/deliveries"
        )))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows = deliveries.as_array().unwrap();
    assert!(!rows.is_empty(), "deliveries were logged: {deliveries}");
    assert_eq!(rows[0]["status_code"], 200, "mock answered 200: {rows:?}");

    // --- disable stops delivery ---------------------------------------------

    let r = ctx
        .sess
        .patch(ctx.url(&format!("/api/environments/{env}/integrations/{slack_id}")))
        .header("X-Checkgate-Request", "true")
        .json(&json!({ "enabled": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "disable integration");

    let before = slack.count();
    ctx.sess
        .post(ctx.url(&format!("/api/environments/{env}/flags")))
        .header("X-Checkgate-Request", "true")
        .json(&json!({
            "key": "chat_notify_two",
            "is_enabled": true,
            "flag_type": "boolean",
            "rules": [],
        }))
        .send()
        .await
        .unwrap();
    // Teams is still enabled, so waiting on it gives the disabled Slack hook a
    // fair chance to have (wrongly) fired before we assert it didn't.
    teams.wait_for(2, "teams still receiving").await;
    assert_eq!(
        slack.count(),
        before,
        "a disabled integration must not receive events"
    );

    // --- delete -------------------------------------------------------------

    let r = ctx
        .sess
        .delete(ctx.url(&format!("/api/environments/{env}/integrations/{slack_id}")))
        .header("X-Checkgate-Request", "true")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204, "delete integration");

    let list: Value = ctx
        .sess
        .get(ctx.url(&format!("/api/environments/{env}/integrations")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !list
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"] == slack_id.as_str()),
        "deleted integration is gone: {list}"
    );

    eprintln!("integration: chat_integrations passed");
}

// Read a response with a useful failure body instead of accepting any 2xx status.
async fn regression_request(
    client: &reqwest::Client,
    base: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let mut request = client.request(method.parse().unwrap(), format!("{base}{path}"));
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    assert_eq!(status, expected, "{method} {path}: {text}");
    if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::Null)
    }
}

async fn security_regressions(ctx: &Ctx, db_url: &str, redis_url: &str, sdk_key: &str, env: &str) {
    use futures_util::StreamExt;
    let pool = sqlx::PgPool::connect(db_url).await.unwrap();
    // These scenarios have their own source IP so the application-wide quota
    // measures the scenario instead of accumulating unrelated API-flow requests.
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("X-Checkgate-Request", "true".parse().unwrap());
    headers.insert("X-Forwarded-For", "198.51.100.222".parse().unwrap());
    let admin = reqwest::Client::builder()
        .cookie_store(true)
        .default_headers(headers.clone())
        .build()
        .unwrap();
    regression_request(
        &admin,
        &ctx.base,
        "POST",
        "/api/auth/login",
        Some(json!({"email":"admin@acme.test","password":"supersecret1"})),
        200,
    )
    .await;
    let editor = reqwest::Client::builder()
        .cookie_store(true)
        .default_headers(headers.clone())
        .build()
        .unwrap();
    let user = regression_request(&admin, &ctx.base, "POST", "/api/users",
        Some(json!({"name":"Scoped Editor","email":"scoped@acme.test","role":"editor","password":"supersecret1"})), 200).await;
    let user_id = user["id"].as_i64().unwrap();
    let project: String =
        sqlx::query_scalar("SELECT project_id::text FROM environments WHERE id = $1::uuid")
            .bind(env)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO project_members (project_id,user_id,role) VALUES ($1::uuid,$2,'viewer')",
    )
    .bind(&project)
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();
    regression_request(
        &editor,
        &ctx.base,
        "POST",
        "/api/auth/login",
        Some(json!({"email":"scoped@acme.test","password":"supersecret1"})),
        200,
    )
    .await;
    regression_request(
        &editor,
        &ctx.base,
        "GET",
        &format!("/api/environments/{env}/flags"),
        None,
        200,
    )
    .await;
    regression_request(
        &editor,
        &ctx.base,
        "PATCH",
        &format!("/api/environments/{env}/flags/gated"),
        Some(json!({"is_enabled":false})),
        403,
    )
    .await;

    let private = regression_request(
        &admin,
        &ctx.base,
        "POST",
        "/api/projects",
        Some(json!({"name":"Private Regression"})),
        200,
    )
    .await;
    let private_id = private["id"].as_str().unwrap();
    let private_env = regression_request(
        &admin,
        &ctx.base,
        "POST",
        &format!("/api/projects/{private_id}/environments"),
        Some(json!({"name":"Private","slug":"private"})),
        200,
    )
    .await;
    let hidden_env = private_env["id"].as_str().unwrap();
    regression_request(
        &admin,
        &ctx.base,
        "POST",
        &format!("/api/environments/{hidden_env}/flags"),
        Some(json!({"key":"secret-regression-flag","is_enabled":true,"rules":[]})),
        200,
    )
    .await;
    let response = editor.get(ctx.url("/stream")).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let mut stream = response.bytes_stream();
    let mut data = String::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !data.contains("event: ready") && !data.contains("event:ready") {
            data.push_str(std::str::from_utf8(&stream.next().await.unwrap().unwrap()).unwrap());
        }
    })
    .await
    .unwrap();
    assert!(
        !data.contains("secret-regression-flag"),
        "private project leaked through bootstrap"
    );
    let mut redis = redis::Client::open(redis_url)
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    for (env_id, key) in [
        (hidden_env, "secret-live-regression"),
        (env, "allowed-live-regression"),
    ] {
        let msg = json!({"type":"UPSERT","env_id":env_id,"flag":{"key":key,"is_enabled":true,"rules":[]}}).to_string();
        redis::cmd("PUBLISH")
            .arg("checkgate_updates")
            .arg(msg)
            .query_async::<i64>(&mut redis)
            .await
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while !data.contains("allowed-live-regression") {
            data.push_str(std::str::from_utf8(&stream.next().await.unwrap().unwrap()).unwrap());
        }
    })
    .await
    .unwrap();
    assert!(
        !data.contains("secret-live-regression"),
        "private project leaked through live deltas"
    );
    drop(stream);

    // Every route that can replace an existing protected flag must honor approval.
    regression_request(
        &admin,
        &ctx.base,
        "POST",
        &format!("/api/environments/{env}/flags"),
        Some(json!({"key":"gated","is_enabled":false,"rules":[]})),
        409,
    )
    .await;
    regression_request(
        &admin,
        &ctx.base,
        "DELETE",
        &format!("/api/environments/{env}/flags/gated"),
        None,
        409,
    )
    .await;
    regression_request(
        &admin,
        &ctx.base,
        "POST",
        &format!("/api/environments/{hidden_env}/flags/secret-regression-flag/promote"),
        Some(json!({"target_env_id":env})),
        409,
    )
    .await;
    let future = "2099-01-01T00:00:00Z";
    regression_request(
        &admin,
        &ctx.base,
        "POST",
        &format!("/api/environments/{env}/flags/gated/scheduled-changes"),
        Some(json!({"scheduled_at":future,"patch":{"is_enabled":false}})),
        409,
    )
    .await;
    regression_request(
        &admin,
        &ctx.base,
        "POST",
        &format!("/api/environments/{env}/segments"),
        Some(json!({"name":"Bypass","key":"bypass","rules":[]})),
        409,
    )
    .await;
    for patch in [json!({"rollout_percentage":101}), json!(null)] {
        regression_request(
            &admin,
            &ctx.base,
            "POST",
            &format!(
                "/api/environments/{hidden_env}/flags/secret-regression-flag/scheduled-changes"
            ),
            Some(json!({"scheduled_at":future,"patch":patch})),
            422,
        )
        .await;
    }

    let scheduled = regression_request(
        &admin,
        &ctx.base,
        "POST",
        &format!("/api/environments/{hidden_env}/flags/secret-regression-flag/scheduled-changes"),
        Some(json!({"scheduled_at":future,"patch":{"is_enabled":false}})),
        200,
    )
    .await;
    assert_eq!(scheduled["attempts"], 0);
    assert!(scheduled["last_error"].is_null());

    // The effective project role is independent of the workspace role.
    sqlx::query("UPDATE project_members SET role='editor' WHERE user_id=$1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET role='viewer' WHERE id=$1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    let queued = regression_request(
        &admin,
        &ctx.base,
        "PATCH",
        &format!("/api/environments/{env}/flags/gated"),
        Some(json!({"is_enabled":false})),
        202,
    )
    .await;
    let id = queued["id"].as_i64().unwrap();
    let approve = format!("/api/environments/{env}/change-requests/{id}/approve");
    regression_request(&admin, &ctx.base, "POST", &approve, None, 403).await;
    let flag: Value = sqlx::query_scalar(
        "DELETE FROM flags WHERE environment_id=$1::uuid AND key='gated' RETURNING data",
    )
    .bind(env)
    .fetch_one(&pool)
    .await
    .unwrap();
    regression_request(&editor, &ctx.base, "POST", &approve, None, 404).await;
    let status: String = sqlx::query_scalar("SELECT status FROM change_requests WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "pending", "failed approval must remain retryable");
    sqlx::query("INSERT INTO flags (environment_id,key,data) VALUES ($1::uuid,'gated',$2)")
        .bind(env)
        .bind(flag)
        .execute(&pool)
        .await
        .unwrap();
    regression_request(&editor, &ctx.base, "POST", &approve, None, 200).await;

    // A browser cannot extend a session by replaying its encrypted cookie after expiry.
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE email='admin@acme.test'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let key = cookie::Key::derive_from(
        b"test-secret-0123456789-0123456789-0123456789-0123456789-0123456789",
    );
    let mut jar = cookie::CookieJar::new();
    jar.private_mut(&key).add(cookie::Cookie::new("lg_session", json!({"user_id":admin_id,"expires_at":0,"email":"admin@acme.test","name":"Admin","role":"admin"}).to_string()));
    let expired = format!("lg_session={}", jar.get("lg_session").unwrap().value());
    let response = ctx
        .anon
        .get(ctx.url("/api/users"))
        .header("Cookie", &expired)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        401,
        "server rejects expired encrypted cookies"
    );
    let victim = regression_request(&admin,&ctx.base,"POST","/api/users",
        Some(json!({"name":"Deleted Admin","email":"deleted@acme.test","role":"admin","password":"supersecret1"})),200).await;
    let deleted = reqwest::Client::builder()
        .cookie_store(true)
        .default_headers(headers.clone())
        .build()
        .unwrap();
    regression_request(
        &deleted,
        &ctx.base,
        "POST",
        "/api/auth/login",
        Some(json!({"email":"deleted@acme.test","password":"supersecret1"})),
        200,
    )
    .await;
    regression_request(
        &admin,
        &ctx.base,
        "DELETE",
        &format!("/api/users/{}", victim["id"]),
        None,
        204,
    )
    .await;
    regression_request(&deleted, &ctx.base, "GET", "/api/users", None, 401).await;
    regression_request(&deleted, &ctx.base, "GET", "/api/auth/me", None, 401).await;

    // Start a second replica BEFORE creating a key, then check creation and revocation there.
    let port = free_port();
    let second_base = format!("http://127.0.0.1:{port}");
    let child = Command::new(env!("CARGO_BIN_EXE_server"))
        .env("DATABASE_URL", db_url)
        .env("REDIS_URL", redis_url)
        .env("PORT", port.to_string())
        .env(
            "SESSION_SECRET",
            "test-secret-0123456789-0123456789-0123456789-0123456789-0123456789",
        )
        .env("COOKIE_SECURE", "false")
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let _second = ServerGuard(child);
    wait_healthy(&second_base).await;
    let created = regression_request(
        &admin,
        &ctx.base,
        "POST",
        &format!("/api/projects/{project}/keys"),
        Some(json!({"name":"Replica Regression","environment_id":env})),
        200,
    )
    .await;
    let response = ctx
        .anon
        .get(format!("{second_base}/flags/snapshot"))
        .bearer_auth(created["key"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "new key works on existing replica");
    assert_eq!(
        response.headers()["x-checkgate-environment-id"]
            .to_str()
            .unwrap(),
        env
    );
    regression_request(
        &admin,
        &ctx.base,
        "DELETE",
        &format!("/api/projects/{project}/keys/{}", created["id"]),
        None,
        204,
    )
    .await;
    let response = ctx
        .anon
        .get(format!("{second_base}/flags/snapshot"))
        .bearer_auth(created["key"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        401,
        "revoked key stops working on other replicas"
    );
    let original: i64 = sqlx::query_scalar("SELECT id FROM sdk_keys WHERE value=$1")
        .bind(sdk_key)
        .fetch_one(&pool)
        .await
        .unwrap();
    regression_request(
        &admin,
        &ctx.base,
        "DELETE",
        &format!("/api/projects/{project}/keys/{original}"),
        None,
        422,
    )
    .await;
    regression_request(
        &admin,
        &ctx.base,
        "DELETE",
        &format!("/api/projects/{private_id}"),
        None,
        204,
    )
    .await;
    regression_request(
        &admin,
        &ctx.base,
        "DELETE",
        &format!("/api/projects/{project}"),
        None,
        422,
    )
    .await;
    pool.close().await;
    eprintln!("integration: security regressions passed");
}
