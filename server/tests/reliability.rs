#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! Multi-instance fault-injection suite.
//!
//! Every test runs two or more real server processes against one real
//! PostgreSQL database and one real Redis, with real HTTP writes and real SSE
//! subscriptions held by simulated SDK clients (`common::sdk`, which follows
//! the shipped SDKs' revision, staging and reconnect rules). Faults are
//! injected with per-instance TCP proxies (`common::chaos`) and by killing
//! processes. Each scenario asserts that every client converges to the state
//! committed in PostgreSQL, and records how long recovery took.
//!
//! Requires CHECKGATE_TEST_DATABASE_URL / CHECKGATE_TEST_REDIS_URL (see
//! `common`); each test creates its own database and Redis channel.

mod common;

use checkgate_core::evaluator::{Flag, UserContext, evaluate};
use checkgate_core::store::FlagStore;
use common::chaos::ChaosProxy;
use common::cluster::{Cluster, flag_body};
use common::sdk::{SdkOptions, SimSdk};
use common::{request, test_env};
use rand::RngExt as _;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Upper bound for every convergence wait. Measured recovery windows are
/// recorded separately and are far below this; the bound only turns a
/// permanent divergence into a failure instead of a hang.
const CONVERGE: Duration = Duration::from_secs(20);

fn eval_bool(sdk: &SimSdk, key: &str) -> bool {
    let (flags, _) = sdk.snapshot();
    let store = FlagStore::new();
    for flag in flags.values() {
        store.upsert_flag(serde_json::from_value::<Flag>(flag.clone()).unwrap());
    }
    let ctx = UserContext {
        key: "user-1".into(),
        attributes: HashMap::new(),
    };
    store
        .get_flag(key)
        .map(|f| evaluate(f.as_ref(), &ctx, &store))
        .unwrap_or(false)
}

async fn wait_db(c: &Cluster, key: &str, enabled: bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if c.authoritative(&c.env_id).await.get(key).map(|v| v.1) == Some(enabled) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "write to {key} never committed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ── A. Normal propagation ────────────────────────────────────────────────────

#[tokio::test]
async fn a_update_on_one_instance_reaches_clients_of_every_instance() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(&env, "rel_a_propagation", 2, &[]).await;
    let sdks = [c.sdk("sdk-a", 0), c.sdk("sdk-b", 1)];
    for sdk in &sdks {
        sdk.wait_readies(1, CONVERGE).await;
    }
    let all: Vec<&SimSdk> = sdks.iter().collect();

    c.create_flag(0, "checkout", false).await;
    let t = c.wait_converged(&all, &c.env_id, CONVERGE).await;
    c.record("create propagates", t);

    for (i, enabled) in [(0, true), (1, false), (0, true)] {
        c.set_enabled(i, "checkout", enabled).await;
        let t = c.wait_converged(&all, &c.env_id, CONVERGE).await;
        c.record("update propagates", t);
        for sdk in &sdks {
            assert_eq!(
                eval_bool(sdk, "checkout"),
                enabled,
                "{} evaluates",
                sdk.name
            );
        }
    }
    // Plain propagation never needs a reconnect.
    for sdk in &sdks {
        assert_eq!(sdk.with(|s| s.connects), 1, "{} reconnected", sdk.name);
    }
}

// ── B. Concurrent updates ────────────────────────────────────────────────────

#[tokio::test]
async fn b_concurrent_writes_through_both_instances_converge_to_the_database() {
    let Some(env) = test_env() else { return };
    let c = Arc::new(Cluster::start(&env, "rel_b_concurrent", 2, &[]).await);
    let keys = ["k0", "k1", "k2"];
    for key in keys {
        c.create_flag(0, key, false).await;
    }
    let sdks = [
        c.sdk("sdk-a1", 0),
        c.sdk("sdk-a2", 0),
        c.sdk("sdk-b1", 1),
        c.sdk("sdk-b2", 1),
    ];
    let all: Vec<&SimSdk> = sdks.iter().collect();
    c.wait_converged(&all, &c.env_id, CONVERGE).await;

    let mut writers = Vec::new();
    for w in 0..8 {
        let c = Arc::clone(&c);
        writers.push(tokio::spawn(async move {
            for n in 0..10 {
                let key = ["k0", "k1", "k2"][(w + n) % 3];
                let enabled = rand::rng().random_bool(0.5);
                assert_eq!(c.try_set_enabled(w % 2, key, enabled).await, 200);
            }
        }));
    }
    for w in writers {
        w.await.unwrap();
    }
    let t = c.wait_converged(&all, &c.env_id, CONVERGE).await;
    c.record("80 concurrent writes", t);
    let dropped: u64 = sdks.iter().map(|s| s.with(|s| s.stale_dropped)).sum();
    eprintln!("stale events dropped by revision check: {dropped}");
}

// ── C. Update/delete races ───────────────────────────────────────────────────

#[tokio::test]
async fn c_racing_update_and_delete_never_resurrects_a_flag() {
    let Some(env) = test_env() else { return };
    let c = Arc::new(Cluster::start(&env, "rel_c_update_delete", 2, &[]).await);
    let sdks = [c.sdk("sdk-a", 0), c.sdk("sdk-b", 1)];
    for sdk in &sdks {
        sdk.wait_readies(1, CONVERGE).await;
    }
    let all: Vec<&SimSdk> = sdks.iter().collect();

    for round in 0..15 {
        let key = format!("race-{round}");
        c.create_flag(round % 2, &key, false).await;
        // Several updates through one instance race one delete through the other.
        let mut tasks = Vec::new();
        for n in 0..3 {
            let (c, key) = (Arc::clone(&c), key.clone());
            tasks.push(tokio::spawn(async move {
                c.try_set_enabled(round % 2, &key, n % 2 == 0).await
            }));
        }
        let (c2, key2) = (Arc::clone(&c), key.clone());
        tasks.push(tokio::spawn(async move {
            c2.try_delete((round + 1) % 2, &key2).await
        }));
        for t in tasks {
            let status = t.await.unwrap();
            assert!(
                status == 200 || status == 204 || status == 404,
                "status {status}"
            );
        }
    }
    let t = c.wait_converged(&all, &c.env_id, CONVERGE).await;
    c.record("update/delete races", t);
    assert!(
        c.authoritative(&c.env_id).await.is_empty(),
        "every raced flag was deleted"
    );

    // Delete, then re-create under the same key through the other instance:
    // the new flag has a higher revision than the tombstone and must appear.
    c.create_flag(0, "phoenix", true).await;
    c.wait_converged(&all, &c.env_id, CONVERGE).await;
    assert!(matches!(c.try_delete(1, "phoenix").await, 200 | 204));
    c.wait_converged(&all, &c.env_id, CONVERGE).await;
    assert_eq!(c.try_create(0, "phoenix", false).await, 200);
    let t = c.wait_converged(&all, &c.env_id, CONVERGE).await;
    c.record("delete then recreate", t);
}

// ── D. Redis unavailable for the whole cluster ───────────────────────────────

#[tokio::test]
async fn d_changes_committed_during_a_redis_outage_reach_every_client() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(&env, "rel_d_redis_outage", 2, &[]).await;
    c.create_flag(0, "outage", false).await;
    let sdks = [c.sdk("sdk-a", 0), c.sdk("sdk-b", 1)];
    let all: Vec<&SimSdk> = sdks.iter().collect();
    c.wait_converged(&all, &c.env_id, CONVERGE).await;

    c.redis(0).refuse();
    c.redis(1).refuse();
    // Writes still commit (PostgreSQL is authoritative) — a Redis outage
    // must not fail them.
    c.set_enabled(0, "outage", true).await;
    c.create_flag(1, "born-in-outage", true).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    c.redis(0).heal();
    c.redis(1).heal();
    let t = c.wait_converged(&all, &c.env_id, CONVERGE).await;
    c.record("redis outage healed", t);
}

// ── E. One instance's subscriber interrupted ─────────────────────────────────

#[tokio::test]
async fn e1_subscriber_outage_on_one_instance_is_resynchronized() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(&env, "rel_e1_subscriber_down", 2, &[]).await;
    c.create_flag(0, "sub", false).await;
    let sdk_b = c.sdk("sdk-b", 1);
    c.wait_converged(&[&sdk_b], &c.env_id, CONVERGE).await;

    c.redis(1).refuse();
    for enabled in [true, false, true] {
        c.set_enabled(0, "sub", enabled).await;
    }
    c.create_flag(0, "sub-new", true).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    c.redis(1).heal();
    let t = c.wait_converged(&[&sdk_b], &c.env_id, CONVERGE).await;
    c.record("subscriber outage healed", t);
}

#[tokio::test]
async fn e2_silently_dead_subscriber_connection_is_detected() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(&env, "rel_e2_half_open", 2, &[]).await;
    c.create_flag(0, "half-open", false).await;
    let sdk_b = c.sdk("sdk-b", 1);
    c.wait_converged(&[&sdk_b], &c.env_id, CONVERGE).await;

    // B's existing Redis connections stop delivering without being closed;
    // new connections work. Nothing ever errors on B's side by itself.
    c.redis(1).strand_existing();
    c.set_enabled(0, "half-open", true).await;
    let t = c.wait_converged(&[&sdk_b], &c.env_id, CONVERGE).await;
    c.record("half-open subscriber", t);
}

// ── F. Process termination ───────────────────────────────────────────────────

#[tokio::test]
async fn f1_killed_instance_restarts_and_its_clients_rebootstrap() {
    let Some(env) = test_env() else { return };
    let mut c = Cluster::start(&env, "rel_f1_kill_restart", 2, &[]).await;
    c.create_flag(0, "restart", false).await;
    let sdks = [c.sdk("sdk-a", 0), c.sdk("sdk-b", 1)];
    let all: Vec<&SimSdk> = sdks.iter().collect();
    c.wait_converged(&all, &c.env_id, CONVERGE).await;

    c.kill(1);
    // Mutations while B is down.
    c.set_enabled(0, "restart", true).await;
    c.create_flag(0, "while-down", true).await;
    c.start_instance(1).await;
    let t = c.wait_converged(&all, &c.env_id, CONVERGE).await;
    c.record("instance killed and restarted", t);
    assert!(
        sdks[1].with(|s| s.readies) >= 2,
        "client of B bootstrapped again"
    );

    // A brand-new client on the restarted instance bootstraps correctly.
    let fresh = c.sdk("sdk-b-new", 1);
    c.wait_converged(&[&fresh], &c.env_id, CONVERGE).await;
}

/// The publishing instance dies after its write committed but before the
/// change event left it. No instance holds the event, so only a mechanism
/// that consults PostgreSQL can bring the other instances' clients back.
#[tokio::test]
async fn f2_publisher_killed_between_commit_and_publish() {
    let Some(env) = test_env() else { return };
    let mut c = Cluster::start(&env, "rel_f2_publisher_crash", 2, &[]).await;
    c.create_flag(0, "lost", false).await;
    let sdk_b = c.sdk("sdk-b", 1);
    c.wait_converged(&[&sdk_b], &c.env_id, CONVERGE).await;

    // Hold A's publish on a dead Redis link so the commit happens and the
    // publish has not finished when A is killed.
    c.redis(0).blackhole();
    let (admin, url) = (
        c.admin.clone(),
        format!("{}/api/environments/{}/flags/lost", c.base(0), c.env_id),
    );
    let write = tokio::spawn(async move {
        let _ = admin
            .patch(url)
            .header("X-Checkgate-Request", "true")
            .json(&json!({ "is_enabled": true }))
            .send()
            .await;
    });
    wait_db(&c, "lost", true).await;
    c.kill(0);
    let _ = write.await;
    c.redis(0).heal();

    let t = c.wait_converged(&[&sdk_b], &c.env_id, CONVERGE).await;
    c.record("publisher killed after commit", t);
}

/// A change committed with no event at all (a crash at the same point, or an
/// operator fixing a row in SQL) reaches connected clients.
#[tokio::test]
async fn f3_change_committed_without_any_event_is_reconciled() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(&env, "rel_f3_silent_commit", 2, &[]).await;
    c.create_flag(0, "silent", false).await;
    c.create_flag(0, "silent-gone", true).await;
    let sdks = [c.sdk("sdk-a", 0), c.sdk("sdk-b", 1)];
    let all: Vec<&SimSdk> = sdks.iter().collect();
    c.wait_converged(&all, &c.env_id, CONVERGE).await;

    sqlx::query(
        "UPDATE flags SET data = jsonb_set(data, '{is_enabled}', 'true') \
         WHERE key = 'silent' AND environment_id = $1::uuid",
    )
    .bind(&c.env_id)
    .execute(&c.pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM flags WHERE key = 'silent-gone' AND environment_id = $1::uuid")
        .bind(&c.env_id)
        .execute(&c.pool)
        .await
        .unwrap();
    let t = c.wait_converged(&all, &c.env_id, CONVERGE).await;
    c.record("commit without event", t);
}

// ── G. Slow consumer ─────────────────────────────────────────────────────────

/// Opens `/stream` over a raw socket with a tiny receive buffer, reads up to
/// the `ready` event, and then stops reading.
async fn stalled_stream(port: u16, key: &str) -> tokio::net::TcpStream {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(4096).unwrap();
    let mut s = socket
        .connect(format!("127.0.0.1:{port}").parse().unwrap())
        .await
        .unwrap();
    s.write_all(
        format!(
            "GET /stream HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {key}\r\n\
             Accept: text/event-stream\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut seen = Vec::new();
    let mut buf = [0u8; 1024];
    while !String::from_utf8_lossy(&seen).contains("event: ready") {
        let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf))
            .await
            .expect("stream bootstraps")
            .unwrap();
        assert!(n > 0, "stream closed during bootstrap");
        seen.extend_from_slice(&buf[..n]);
    }
    s
}

async fn connection_count(c: &Cluster, i: usize) -> usize {
    request(
        &c.admin,
        "GET",
        &format!("{}/api/health/connections", c.base(i)),
        None,
        200,
    )
    .await
    .as_array()
    .unwrap()
    .len()
}

#[tokio::test]
async fn g_slow_consumer_is_disconnected_and_others_keep_converging() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(&env, "rel_g_slow_consumer", 2, &[]).await;
    // ~48 KB per change event, so a few hundred events overflow both the
    // socket buffers and the broadcast channel (capacity 256) behind them.
    let big = "x".repeat(48 * 1024);
    let mut body = flag_body("big", false);
    body["description"] = json!(big);
    c.create_flag_body(0, &c.env_id.clone(), body).await;
    let fast = c.sdk("fast", 1);
    c.wait_converged(&[&fast], &c.env_id, CONVERGE).await;

    let mut slow = stalled_stream(c.instances[1].port, &c.sdk_key).await;
    let before = connection_count(&c, 1).await;
    assert_eq!(before, 2, "fast and slow are connected");

    for n in 0..600 {
        c.set_enabled(n % 2, "big", n % 2 == 0).await;
    }
    let t = c.wait_converged(&[&fast], &c.env_id, CONVERGE).await;
    c.record("fast client beside a stalled one", t);

    // The stalled client resumes reading: it must find its stream ended
    // (the lag was detected) rather than continue on a gapped event sequence.
    let ended = tokio::time::timeout(Duration::from_secs(30), async {
        let mut tail = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = slow.read(&mut buf).await.unwrap_or(0);
            if n == 0 {
                return true;
            }
            tail.extend_from_slice(&buf[..n]);
            if tail.len() > 16 {
                tail.drain(..tail.len() - 16);
            }
            // Chunked transfer: a zero-length chunk ends the response body.
            if tail.ends_with(b"\r\n0\r\n\r\n") {
                return true;
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(ended, "the lagging stream was closed by the server");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while connection_count(&c, 1).await != 1 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "slow client still registered"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ── H. Bootstrap racing writes ───────────────────────────────────────────────

#[tokio::test]
async fn h_bootstraps_racing_writes_never_settle_on_stale_state() {
    let Some(env) = test_env() else { return };
    let c = Arc::new(Cluster::start(&env, "rel_h_bootstrap_race", 2, &[]).await);
    for k in 0..8 {
        c.create_flag(k % 2, &format!("h{k}"), false).await;
    }
    // Clients reach the servers through proxies, so their streams can be cut
    // (forcing a fresh bootstrap) while writes are in flight.
    let mut proxies = Vec::new();
    let mut sdks = Vec::new();
    for n in 0..6 {
        let proxy = ChaosProxy::start(&format!("127.0.0.1:{}", c.instances[n % 2].port)).await;
        sdks.push(SimSdk::start(
            &format!("sdk-{n}"),
            &format!("http://127.0.0.1:{}", proxy.port),
            &c.sdk_key,
        ));
        proxies.push(proxy);
    }

    let writer = {
        let c = Arc::clone(&c);
        tokio::spawn(async move {
            for n in 0..200usize {
                let key = format!("h{}", n % 8);
                let via = n % 2;
                let (op, enabled) = {
                    let mut rng = rand::rng();
                    (rng.random_range(0..10), rng.random_bool(0.5))
                };
                match op {
                    0 => {
                        c.try_delete(via, &key).await;
                    }
                    1 => {
                        c.try_create(via, &key, true).await;
                    }
                    _ => {
                        c.try_set_enabled(via, &key, enabled).await;
                    }
                }
            }
        })
    };
    while !writer.is_finished() {
        let i = rand::rng().random_range(0..proxies.len());
        proxies[i].sever();
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    writer.await.unwrap();

    let all: Vec<&SimSdk> = sdks.iter().collect();
    let t = c.wait_converged(&all, &c.env_id, CONVERGE).await;
    c.record("bootstraps racing 200 writes", t);
    let readies: u32 = sdks.iter().map(|s| s.with(|s| s.readies)).sum();
    assert!(
        readies > 6,
        "streams were re-bootstrapped during the writes ({readies})"
    );
}

// ── I. Network interruption between SDK and server ──────────────────────────

#[tokio::test]
async fn i_dropped_client_connections_reconnect_or_fall_back_to_polling() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(&env, "rel_i_network", 2, &[]).await;
    c.create_flag(0, "net", false).await;
    let link = ChaosProxy::start(&format!("127.0.0.1:{}", c.instances[1].port)).await;
    let via_link = format!("http://127.0.0.1:{}", link.port);
    let sdk = SimSdk::start("sdk-link", &via_link, &c.sdk_key);
    c.wait_converged(&[&sdk], &c.env_id, CONVERGE).await;

    // Dropped connections: reconnect and re-bootstrap.
    for enabled in [true, false, true] {
        link.sever();
        c.set_enabled(0, "net", enabled).await;
        let t = c.wait_converged(&[&sdk], &c.env_id, CONVERGE).await;
        c.record("client link severed", t);
    }

    // Link down for a while with changes in between.
    link.refuse();
    c.set_enabled(0, "net", false).await;
    c.create_flag(0, "net-2", true).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    link.heal();
    let t = c.wait_converged(&[&sdk], &c.env_id, CONVERGE).await;
    c.record("client link down 2s", t);

    // Streaming blocked but plain requests allowed (a proxy that kills
    // long-lived connections): the client converges by polling snapshots.
    let blocked = ChaosProxy::start(&format!("127.0.0.1:{}", c.instances[1].port)).await;
    blocked.refuse();
    let poller = SimSdk::start_with(
        "sdk-poller",
        &format!("http://127.0.0.1:{}", blocked.port),
        &c.sdk_key,
        SdkOptions {
            poll_base: Some(c.base(1)),
            ..SdkOptions::default()
        },
    );
    c.set_enabled(0, "net-2", false).await;
    let t = c.wait_converged(&[&poller], &c.env_id, CONVERGE).await;
    c.record("poll fallback", t);
    assert!(
        poller.with(|s| s.polling && s.polls > 0),
        "client is polling"
    );
    c.set_enabled(1, "net", true).await;
    c.wait_converged(&[&poller], &c.env_id, CONVERGE).await;
}

// ── J. Environment isolation across instances ────────────────────────────────

#[tokio::test]
async fn j_identical_keys_in_two_environments_never_cross() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(&env, "rel_j_isolation", 2, &[]).await;
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
        Some(json!({ "name": "Isolation", "slug": "isolation-check" })),
        200,
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let key2 = request(
        &c.admin,
        "POST",
        &format!("{}/api/projects/{project}/keys", c.base(1)),
        Some(json!({ "name": "staging", "environment_id": env2 })),
        200,
    )
    .await["key"]
        .as_str()
        .unwrap()
        .to_string();

    c.create_flag(0, "shared", true).await;
    c.create_flag_body(1, &env2, flag_body("shared", false))
        .await;

    let prod = [c.sdk("prod-a", 0), c.sdk("prod-b", 1)];
    let staging = [
        SimSdk::start("staging-a", &c.base(0), &key2),
        SimSdk::start("staging-b", &c.base(1), &key2),
    ];
    let prod_refs: Vec<&SimSdk> = prod.iter().collect();
    let staging_refs: Vec<&SimSdk> = staging.iter().collect();
    c.wait_converged(&prod_refs, &c.env_id, CONVERGE).await;
    c.wait_converged(&staging_refs, &env2, CONVERGE).await;

    for (i, enabled) in [(1, false), (0, true), (1, true)] {
        c.set_enabled(i, "shared", enabled).await;
        c.create_flag(i, &format!("prod-only-{i}-{enabled}"), true)
            .await;
    }
    c.wait_converged(&prod_refs, &c.env_id, CONVERGE).await;
    c.wait_converged(&staging_refs, &env2, CONVERGE).await;

    for sdk in &staging {
        let seen = sdk.with(|s| s.seen_envs.clone());
        assert!(
            seen.iter().all(|e| *e == env2),
            "{} saw events of {seen:?}",
            sdk.name
        );
        assert!(
            !eval_bool(sdk, "shared"),
            "{} evaluates its own environment",
            sdk.name
        );
    }
    for sdk in &prod {
        let seen = sdk.with(|s| s.seen_envs.clone());
        assert!(
            seen.iter().all(|e| *e == c.env_id),
            "{} saw events of {seen:?}",
            sdk.name
        );
    }
    for i in 0..2 {
        let snap: Vec<Value> = reqwest::Client::new()
            .get(format!("{}/flags/snapshot", c.base(i)))
            .bearer_auth(&key2)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(snap.len(), 1, "staging snapshot holds only its own flag");
        assert_eq!(snap[0]["is_enabled"], false);
    }
}

/// A client that stops reading for good (a hung process, a peer whose network
/// vanished) must not hold its connection, task and tracking entry forever.
/// While its socket is full the server never polls the stream, so only a
/// transport-level timeout can release it.
#[tokio::test]
async fn g2_client_that_never_reads_again_is_released() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(
        &env,
        "rel_g2_dead_reader",
        1,
        &[("TCP_USER_TIMEOUT_SECS", Some("5"))],
    )
    .await;
    let mut body = flag_body("big", false);
    body["description"] = json!("x".repeat(48 * 1024));
    c.create_flag_body(0, &c.env_id.clone(), body).await;
    let _stalled = stalled_stream(c.instances[0].port, &c.sdk_key).await;
    assert_eq!(connection_count(&c, 0).await, 1);
    for n in 0..200 {
        c.set_enabled(0, "big", n % 2 == 0).await;
    }
    let start = std::time::Instant::now();
    while connection_count(&c, 0).await != 0 {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "a client that never reads still holds its stream after 60 s"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    c.record("dead reader released", start.elapsed());
}
