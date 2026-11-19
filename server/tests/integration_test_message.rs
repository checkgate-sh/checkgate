#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! The chat integration "Send test message" endpoint
//! (`POST /api/environments/{env}/integrations/{id}/test`), against a real
//! server and real sockets:
//!
//! - the test reaches the selected integration and no other one, whatever the
//!   other integrations subscribe to;
//! - it reaches the selected integration even when that integration is
//!   disabled or does not subscribe to `flag.updated` (an explicit test of the
//!   wiring, not an environment event);
//! - the response reports what the receiver did: delivered with its status,
//!   or not delivered with the status or a URL-free error (refused
//!   connection, timeout);
//! - the attempt is logged in the integration's delivery log as `test`;
//! - unknown, malformed and other-environment ids are 404; only admins may
//!   send a test.
//!
//! Every case is observed first and asserted at the end, so a run against an
//! older binary (`CHECKGATE_TEST_SERVER_BIN`) prints the whole picture.

mod common;

use common::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How a mock receiver answers.
#[derive(Clone, Copy)]
enum Reply {
    Status(u16),
    /// Accept the request and never answer.
    Hang,
}

/// A stand-in for a Slack or Teams incoming webhook that counts what it gets.
struct MockChat {
    url: String,
    received: Arc<Mutex<Vec<Value>>>,
}

impl MockChat {
    async fn start(reply: Reply) -> Self {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let received: Arc<Mutex<Vec<Value>>> = Arc::default();
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
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&tmp[..n]),
                        }
                        let Some(head) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                            continue;
                        };
                        let head = head + 4;
                        let len = String::from_utf8_lossy(&buf[..head])
                            .to_lowercase()
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if buf.len() >= head + len {
                            if let Ok(v) = serde_json::from_slice(&buf[head..head + len]) {
                                sink.lock().unwrap().push(v);
                            }
                            break;
                        }
                    }
                    match reply {
                        Reply::Status(code) => {
                            let _ = socket
                                .write_all(
                                    format!("HTTP/1.1 {code} X\r\nContent-Length: 2\r\n\r\nok")
                                        .as_bytes(),
                                )
                                .await;
                        }
                        Reply::Hang => tokio::time::sleep(Duration::from_secs(60)).await,
                    }
                });
            }
        });
        MockChat {
            url: format!("http://127.0.0.1:{port}/hook"),
            received,
        }
    }

    fn count(&self) -> usize {
        self.received.lock().unwrap().len()
    }
}

/// One test request: HTTP status, parsed body, how long it took.
struct Outcome {
    status: u16,
    body: Value,
    elapsed: Duration,
}

async fn send_test(client: &reqwest::Client, url: &str) -> Outcome {
    let started = Instant::now();
    let r = client
        .post(url)
        .header("X-Checkgate-Request", "true")
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    Outcome {
        status,
        body: serde_json::from_str(&text).unwrap_or(Value::Null),
        elapsed: started.elapsed(),
    }
}

#[tokio::test]
async fn test_message_reaches_only_the_selected_integration_and_reports_the_result() {
    let Some(env) = test_env() else {
        return;
    };
    let db = env.fresh_database("cg_integration_test_message").await;
    let server = Server::spawn(&db, &env.redis_url, &[]).await;
    let (admin, sdk_key, env_id) = complete_setup(&server).await;
    let base = server.url(&format!("/api/environments/{env_id}/integrations"));

    let selected = MockChat::start(Reply::Status(200)).await;
    let bystander = MockChat::start(Reply::Status(200)).await;
    let disabled = MockChat::start(Reply::Status(200)).await;
    let deletes_only = MockChat::start(Reply::Status(200)).await;
    let rejecting = MockChat::start(Reply::Status(404)).await;
    let hanging = MockChat::start(Reply::Hang).await;
    let refused = format!("http://127.0.0.1:{}/hook", free_port());

    let mut ids = std::collections::HashMap::new();
    for (name, kind, url, events, enabled) in [
        ("selected", "slack", selected.url.as_str(), json!([]), true),
        (
            "bystander",
            "teams",
            bystander.url.as_str(),
            json!([]),
            true,
        ),
        ("disabled", "slack", disabled.url.as_str(), json!([]), false),
        (
            "deletes_only",
            "slack",
            deletes_only.url.as_str(),
            json!(["flag.deleted"]),
            true,
        ),
        (
            "rejecting",
            "slack",
            rejecting.url.as_str(),
            json!([]),
            true,
        ),
        ("hanging", "teams", hanging.url.as_str(), json!([]), true),
        ("refused", "slack", refused.as_str(), json!([]), true),
    ] {
        let row = request(
            &admin,
            "POST",
            &base,
            Some(json!({ "kind": kind, "name": name, "webhook_url": url, "events": events, "enabled": enabled })),
            200,
        )
        .await;
        ids.insert(name, row["id"].as_str().unwrap().to_string());
    }
    let test_url = |name: &str| format!("{base}/{}/test", ids[name]);

    // Observe every case first.
    let ok = send_test(&admin, &test_url("selected")).await;
    // Give a broadcast (the old behaviour) time to reach the bystanders.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let counts_after_selected = (
        selected.count(),
        bystander.count(),
        disabled.count(),
        deletes_only.count(),
    );
    let disabled_out = send_test(&admin, &test_url("disabled")).await;
    let filtered_out = send_test(&admin, &test_url("deletes_only")).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let counts_after_all = (disabled.count(), deletes_only.count(), bystander.count());
    let rejected_out = send_test(&admin, &test_url("rejecting")).await;
    let refused_out = send_test(&admin, &test_url("refused")).await;
    let hanging_out = send_test(&admin, &test_url("hanging")).await;

    let unknown = send_test(
        &admin,
        &format!("{base}/00000000-0000-0000-0000-000000000000/test"),
    )
    .await;
    let malformed = send_test(&admin, &format!("{base}/not-a-uuid/test")).await;

    // Another environment of the same project must not reach this integration.
    let projects = request(&admin, "GET", &server.url("/api/projects"), None, 200).await;
    let project_id = projects[0]["id"].as_str().unwrap();
    let envs = request(
        &admin,
        "GET",
        &server.url(&format!("/api/projects/{project_id}/environments")),
        None,
        200,
    )
    .await;
    let other_env = envs
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .find(|id| *id != env_id)
        .expect("setup creates more than one environment");
    let cross_env = send_test(
        &admin,
        &server.url(&format!(
            "/api/environments/{other_env}/integrations/{}/test",
            ids["selected"]
        )),
    )
    .await;

    // Authorization: anonymous, the public SDK key, a viewer and an editor.
    let anonymous = send_test(&reqwest::Client::new(), &test_url("selected")).await;
    let with_sdk_key = send_test(&bearer_client(&sdk_key), &test_url("selected")).await;
    let mut role_outcomes = Vec::new();
    for role in ["viewer", "editor"] {
        let email = format!("{role}@acme.test");
        request(
            &admin,
            "POST",
            &server.url("/api/users"),
            Some(json!({ "name": role, "email": email, "role": role, "password": "supersecret1" })),
            200,
        )
        .await;
        let client = login(&server, &email, "supersecret1").await;
        role_outcomes.push((role, send_test(&client, &test_url("selected")).await));
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let selected_total = selected.count();

    let deliveries = request(
        &admin,
        "GET",
        &format!("{base}/{}/deliveries", ids["rejecting"]),
        None,
        200,
    )
    .await;

    let show = |label: &str, o: &Outcome| {
        eprintln!(
            "{label:>14}: HTTP {} in {:>5} ms, body {}",
            o.status,
            o.elapsed.as_millis(),
            o.body
        );
    };
    show("selected", &ok);
    eprintln!(
        "    received after testing `selected` (selected, bystander, disabled, deletes_only): {counts_after_selected:?}"
    );
    show("disabled", &disabled_out);
    show("deletes_only", &filtered_out);
    eprintln!("    received (disabled, deletes_only, bystander): {counts_after_all:?}");
    show("rejecting", &rejected_out);
    show("refused", &refused_out);
    show("hanging", &hanging_out);
    show("unknown id", &unknown);
    show("malformed id", &malformed);
    show("other env", &cross_env);
    show("anonymous", &anonymous);
    show("sdk key", &with_sdk_key);
    for (role, o) in &role_outcomes {
        show(role, o);
    }
    eprintln!("    `rejecting` delivery log: {deliveries}");
    eprintln!("    `selected` received in total: {selected_total}");

    // The selected integration gets exactly one message; nobody else does.
    assert_eq!(ok.status, 200, "test of a working integration");
    assert_eq!(ok.body["delivered"], true, "{}", ok.body);
    assert_eq!(ok.body["status_code"], 200, "{}", ok.body);
    assert_eq!(
        counts_after_selected,
        (1, 0, 0, 0),
        "only the selected integration receives its test message"
    );

    // Disabled, or not subscribed to flag.updated: still tested, alone.
    for (label, o) in [("disabled", &disabled_out), ("deletes_only", &filtered_out)] {
        assert_eq!(o.status, 200, "{label}");
        assert_eq!(o.body["delivered"], true, "{label}: {}", o.body);
    }
    assert_eq!(counts_after_all, (1, 1, 0));

    // Failures are reported as failures, with no URL in the error.
    assert_eq!(rejected_out.status, 200);
    assert_eq!(
        rejected_out.body["delivered"], false,
        "{}",
        rejected_out.body
    );
    assert_eq!(
        rejected_out.body["status_code"], 404,
        "{}",
        rejected_out.body
    );
    for (label, o) in [("refused", &refused_out), ("hanging", &hanging_out)] {
        assert_eq!(o.status, 200, "{label}");
        assert_eq!(o.body["delivered"], false, "{label}: {}", o.body);
        assert!(o.body["status_code"].is_null(), "{label}: {}", o.body);
        let error = o.body["error"].as_str().unwrap_or_default();
        assert!(!error.is_empty(), "{label}: an error is reported");
        assert!(
            !error.contains("127.0.0.1") && !error.contains("/hook"),
            "{label}: the error must not carry the webhook URL: {error}"
        );
    }
    assert!(
        hanging_out.body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("timed out"),
        "{}",
        hanging_out.body
    );
    // One attempt, bounded by the delivery client's 10 s timeout (no retries).
    assert!(
        refused_out.elapsed < Duration::from_secs(5),
        "refused: no retry backoff"
    );
    assert!(
        hanging_out.elapsed < Duration::from_secs(15),
        "hanging: bounded"
    );

    // The attempt is in the delivery log, as a test.
    let rows = deliveries.as_array().unwrap();
    assert_eq!(rows.len(), 1, "{deliveries}");
    assert_eq!(rows[0]["event"], "test");
    assert_eq!(rows[0]["status_code"], 404);

    // Ids: unknown, malformed and another environment's are all 404.
    for (label, o) in [
        ("unknown id", &unknown),
        ("malformed id", &malformed),
        ("other env", &cross_env),
    ] {
        assert_eq!(o.status, 404, "{label}: {}", o.body);
    }

    // Only admins: nothing else reached the receiver.
    assert_eq!(anonymous.status, 401);
    assert!(
        [401, 403].contains(&with_sdk_key.status),
        "sdk key: {}",
        with_sdk_key.status
    );
    for (role, o) in &role_outcomes {
        assert_eq!(o.status, 403, "{role}");
    }
    assert_eq!(selected_total, 1, "rejected requests sent nothing");
}
