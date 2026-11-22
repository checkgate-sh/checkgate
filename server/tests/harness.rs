#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! The test harness's own guarantees (tests/common), against the real binary.
//!
//! `reliability::g2` once failed with `Address already in use`: the harness
//! picked a free port, released it, and started the server on it; another
//! test started its server on the same port first, the second server failed
//! to bind, and the harness — polling `/health` on that port — got the first
//! server's 200 and handed the test the wrong instance (its `setup/complete`
//! then answered 404). Servers now bind port 0 and the harness reads the
//! address from the process's own log, so:
//!
//! - parallel spawns always get distinct ports, each answered by its own
//!   process;
//! - a server that cannot bind is never mistaken for whatever does answer on
//!   the port: the harness retries a bounded number of times and then fails
//!   with the process's log.

mod common;

use common::*;
use std::io::{Read, Write};
use std::time::Duration;

/// Answers every request with `200 OK` and an empty JSON body — a stand-in for
/// another test's server holding the port.
fn impostor() -> (u16, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            match listener.accept() {
                Ok((mut s, _)) => {
                    let mut buf = [0u8; 1024];
                    let _ = s.read(&mut buf);
                    let _ = s.write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
                    );
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    });
    (port, handle)
}

#[tokio::test]
async fn parallel_spawns_get_distinct_ports_answered_by_their_own_process() {
    let Some(env) = test_env() else { return };
    let mut handles = Vec::new();
    for i in 0..6 {
        let db = env.fresh_database(&format!("cg_harness_par_{i}")).await;
        let redis = env.redis_url.clone();
        handles.push(tokio::spawn(async move {
            Server::spawn(&db, &redis, &[]).await
        }));
    }
    let mut servers = Vec::new();
    for h in handles {
        servers.push(h.await.unwrap());
    }
    let mut ports: Vec<u16> = servers.iter().map(Server::port).collect();
    ports.sort_unstable();
    ports.dedup();
    assert_eq!(ports.len(), servers.len(), "ports {ports:?}");
    for s in &servers {
        // Each log names the port the harness reports: the process that
        // answers is the process the test holds.
        assert!(
            s.log().contains(&format!(":{}\"", s.port())),
            "log of {} does not show its port",
            s.base
        );
        complete_setup(s).await;
    }
}

#[tokio::test]
async fn a_server_that_cannot_bind_is_never_mistaken_for_what_answers_there() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_harness_impostor").await;
    let (port, _impostor) = impostor();
    // Something answers /health with 200 on `port`; the old harness returned
    // that as a healthy server. This must fail, after bounded retries, with
    // the process's own log.
    let redis = env.redis_url.clone();
    let attempt = tokio::spawn(async move {
        Server::spawn_in(&unique_dir("impostor"), port, &db, &redis, &[]).await;
    });
    let err = attempt
        .await
        .expect_err("spawn_in returned a server on a port it could not bind");
    let msg = err
        .into_panic()
        .downcast::<String>()
        .map(|m| *m)
        .unwrap_or_default();
    assert!(
        msg.contains(&format!("could not bind port {port}")),
        "{msg}"
    );
    assert!(
        msg.contains("Failed to bind TCP listener"),
        "carries the log: {msg}"
    );
}
