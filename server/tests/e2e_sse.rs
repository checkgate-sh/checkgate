#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! End-to-end test of the live evaluation path: a real server, a real SSE
//! connection, and the real shared `checkgate-core` evaluation engine — the
//! exact code every SDK (Node/Web/RN/Flutter) wraps.
//!
//! It proves the full server → SDK loop that the REST integration test can't:
//! a flag change written over REST is pushed over `/stream` in real time, loaded
//! into a local `FlagStore`, and evaluated locally with the correct result.
//!
//! Same env gating as the integration test (skips cleanly if unset):
//!   CHECKGATE_TEST_DATABASE_URL, CHECKGATE_TEST_REDIS_URL

use checkgate_core::evaluator::{Flag, UserContext, evaluate};
use checkgate_core::store::FlagStore;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::sync::mpsc;

/// Reads the two env vars the suite needs, or signals that it should be skipped.
///
/// Locally, an unset pair means "no database here" and the test skips. In CI it
/// means the workflow stopped providing them — which would otherwise turn this
/// whole suite into a silent no-op that still reports `ok`. So when `CI` is set,
/// a missing variable is a hard failure instead of a skip.
fn test_env() -> Option<(String, String)> {
    match (
        std::env::var("CHECKGATE_TEST_DATABASE_URL"),
        std::env::var("CHECKGATE_TEST_REDIS_URL"),
    ) {
        (Ok(db), Ok(redis)) => Some((db, redis)),
        _ => {
            assert!(
                std::env::var("CI").is_err(),
                "CHECKGATE_TEST_DATABASE_URL / CHECKGATE_TEST_REDIS_URL are unset under CI — \
                 the integration suite would skip silently and report success. Check the \
                 `env:` block of the server-integration job."
            );
            eprintln!(
                "SKIP: set CHECKGATE_TEST_DATABASE_URL and CHECKGATE_TEST_REDIS_URL to run server integration tests"
            );
            None
        }
    }
}

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
        .expect("connect test db");
    sqlx::query("DROP SCHEMA public CASCADE")
        .execute(&pool)
        .await
        .expect("drop schema");
    sqlx::query("CREATE SCHEMA public")
        .execute(&pool)
        .await
        .expect("create schema");
    pool.close().await;
}

async fn wait_healthy(base: &str) {
    let client = reqwest::Client::new();
    for _ in 0..120 {
        if let Ok(r) = client.get(format!("{base}/health")).send().await
            && r.status().is_success()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("server never became healthy");
}

/// A parsed SSE frame: (event name, data payload).
type SseEvent = (String, String);

/// Spawn a task that connects to `/stream` with the SDK key and forwards each
/// parsed SSE frame to the returned channel. Mirrors what an SDK's EventSource
/// does — bootstrap replay, then live UPSERT/DELETE deltas.
fn open_stream(base: &str, sdk_key: &str) -> mpsc::UnboundedReceiver<SseEvent> {
    let (tx, rx) = mpsc::unbounded_channel();
    let url = format!("{base}/stream");
    let key = sdk_key.to_string();
    tokio::spawn(async move {
        let resp = reqwest::Client::new()
            .get(&url)
            .bearer_auth(&key)
            .header("Accept", "text/event-stream")
            .send()
            .await
            .expect("open sse");
        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        let (mut event, mut data) = (String::new(), String::new());
        while let Some(Ok(chunk)) = stream.next().await {
            buf.push_str(&String::from_utf8_lossy(&chunk));
            // Frames are separated by a blank line; fields are `name: value`.
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
                // `:`-prefixed comment lines (keep-alives) are ignored.
            }
        }
    });
    rx
}

/// Await the next event of a given name, applying any UPSERT/DELETE deltas seen
/// along the way to `store`. Times out so a missing event fails instead of hanging.
async fn pump_until(
    rx: &mut mpsc::UnboundedReceiver<SseEvent>,
    store: &FlagStore,
    want_event: &str,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let (event, data) = tokio::time::timeout(remaining, rx.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for SSE event '{want_event}'"))
            .expect("sse channel closed");
        apply_event(store, &event, &data);
        if event == want_event {
            return;
        }
    }
}

/// Apply an SSE `update` frame to the local store, exactly as an SDK would.
fn apply_event(store: &FlagStore, event: &str, data: &str) {
    if event != "update" {
        return;
    }
    let Ok(msg) = serde_json::from_str::<Value>(data) else {
        return;
    };
    match msg["type"].as_str() {
        Some("UPSERT") => {
            if let Ok(flag) = serde_json::from_value::<Flag>(msg["flag"].clone()) {
                store.upsert_flag(flag);
            }
        }
        Some("DELETE") => {
            if let Some(key) = msg["key"].as_str() {
                store.delete_flag(key);
            }
        }
        _ => {}
    }
}

fn eval(store: &FlagStore, key: &str) -> bool {
    match store.get_flag(key) {
        Some(flag) => {
            let ctx = UserContext {
                key: "user-1".into(),
                attributes: HashMap::new(),
            };
            evaluate(flag.as_ref(), &ctx, store)
        }
        None => false,
    }
}

#[tokio::test]
async fn sse_live_push_and_local_eval() {
    let Some((db_url, redis_url)) = test_env() else {
        return;
    };

    reset_schema(&db_url).await;

    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let public_dir = std::env::temp_dir().join(format!("checkgate-e2e-public-{port}"));
    std::fs::create_dir_all(&public_dir).unwrap();

    let child = Command::new(env!("CARGO_BIN_EXE_server"))
        .env("DATABASE_URL", &db_url)
        .env("REDIS_URL", &redis_url)
        .env("PORT", port.to_string())
        .env(
            "SESSION_SECRET",
            "e2e-secret-0123456789-0123456789-0123456789-0123456789",
        )
        .env("COOKIE_SECURE", "false")
        .env("PUBLIC_DIR", public_dir.to_str().unwrap())
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn server");
    let _guard = ServerGuard(child);
    wait_healthy(&base).await;

    let anon = reqwest::Client::new();

    // Grab the seeded SDK key + env, complete setup.
    let key_body: Value = anon
        .get(format!("{base}/api/setup/key"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sdk_key = key_body["key"].as_str().unwrap().to_string();
    let env = key_body["environment_id"].as_str().unwrap().to_string();
    anon.post(format!("{base}/api/setup/complete"))
        .header("X-Checkgate-Request", "true")
        .json(&json!({ "workspace_name": "E2E", "project_name": "P", "name": "A", "email": "a@e.test", "password": "supersecret1" }))
        .send().await.unwrap();

    let flags_url = format!("{base}/api/environments/{env}/flags");

    // A flag that exists before the SDK connects — it must arrive in the bootstrap replay.
    anon.post(&flags_url).bearer_auth(&sdk_key)
        .json(&json!({ "key": "bootstrap-flag", "is_enabled": true, "rollout_percentage": 100, "flag_type": "boolean", "rules": [] }))
        .send().await.unwrap();

    // Connect the "SDK": open SSE and load the replayed flags into a real core store.
    let store = FlagStore::new();
    let mut rx = open_stream(&base, &sdk_key);
    pump_until(&mut rx, &store, "ready").await;

    // Bootstrap replay delivered the pre-existing flag; local eval sees it.
    assert!(
        eval(&store, "bootstrap-flag"),
        "bootstrap flag should evaluate true after replay"
    );

    // Now push a NEW flag over REST — it must arrive as a live SSE update.
    anon.post(&flags_url).bearer_auth(&sdk_key)
        .json(&json!({ "key": "pushed-flag", "is_enabled": true, "rollout_percentage": 100, "flag_type": "boolean", "rules": [] }))
        .send().await.unwrap();
    pump_until(&mut rx, &store, "update").await;
    assert!(
        eval(&store, "pushed-flag"),
        "live-pushed flag should evaluate true"
    );

    // Disable it via REST — the update propagates and local eval flips to false.
    anon.patch(format!("{flags_url}/pushed-flag"))
        .bearer_auth(&sdk_key)
        .json(&json!({ "is_enabled": false }))
        .send()
        .await
        .unwrap();
    pump_until(&mut rx, &store, "update").await;
    assert!(
        !eval(&store, "pushed-flag"),
        "disabled flag should evaluate false"
    );

    // Delete it — the DELETE removes it from the local store entirely.
    anon.delete(format!("{flags_url}/pushed-flag"))
        .bearer_auth(&sdk_key)
        .send()
        .await
        .unwrap();
    pump_until(&mut rx, &store, "update").await;
    assert!(
        store.get_flag("pushed-flag").is_none(),
        "deleted flag should be gone from the local store"
    );

    eprintln!("e2e: sse_live_push_and_local_eval passed");
}
