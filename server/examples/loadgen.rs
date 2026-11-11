#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "benchmark harness, not shipped code"
)]
//! Control-plane load generator: SSE connection ramp-up, held connections,
//! update propagation, snapshot throughput and REST latency against real
//! server processes, PostgreSQL and Redis.
//!
//! It starts the server instances itself (so their CPU and memory can be read
//! from /proc), drives them from this process, and timestamps every send and
//! every receive on this process's monotonic clock — propagation latency is
//! free of clock skew. The generator shares the machine with the servers;
//! pin them apart with `taskset` for steadier numbers.
//!
//!     cargo build --release -p server --example loadgen --bin server
//!     target/release/examples/loadgen \
//!         --server-bin target/release/server \
//!         --database-url postgres://postgres:postgres@localhost:5432/postgres \
//!         --redis-url redis://localhost:6379 \
//!         --connections 100,1000 --updates 200 --out results.json
//!
//! See docs/performance/benchmark-methodology.md.

use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

struct Args {
    server_bin: String,
    database_url: String,
    redis_url: String,
    connections: Vec<usize>,
    instances: usize,
    updates: usize,
    flags: usize,
    snapshot_secs: u64,
    snapshot_concurrency: usize,
    db_max_connections: u32,
    out: Option<String>,
}

fn args() -> Args {
    let mut a = Args {
        server_bin: "target/release/server".into(),
        database_url: std::env::var("CHECKGATE_TEST_DATABASE_URL").unwrap_or_default(),
        redis_url: std::env::var("CHECKGATE_TEST_REDIS_URL").unwrap_or_default(),
        connections: vec![100, 1000],
        instances: 2,
        updates: 200,
        flags: 100,
        snapshot_secs: 10,
        snapshot_concurrency: 32,
        db_max_connections: 10,
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut v = || it.next().unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--server-bin" => a.server_bin = v(),
            "--database-url" => a.database_url = v(),
            "--redis-url" => a.redis_url = v(),
            "--connections" => {
                a.connections = v().split(',').map(|n| n.trim().parse().unwrap()).collect()
            }
            "--instances" => a.instances = v().parse().unwrap(),
            "--updates" => a.updates = v().parse().unwrap(),
            "--flags" => a.flags = v().parse().unwrap(),
            "--snapshot-secs" => a.snapshot_secs = v().parse().unwrap(),
            "--snapshot-concurrency" => a.snapshot_concurrency = v().parse().unwrap(),
            "--db-max-connections" => a.db_max_connections = v().parse().unwrap(),
            "--out" => a.out = Some(v()),
            other => panic!("unknown argument {other}"),
        }
    }
    a.server_bin = std::fs::canonicalize(&a.server_bin)
        .unwrap_or_else(|e| panic!("--server-bin {}: {e}", a.server_bin))
        .to_string_lossy()
        .into_owned();
    assert!(
        !a.database_url.is_empty(),
        "--database-url (or CHECKGATE_TEST_DATABASE_URL) is required"
    );
    assert!(
        !a.redis_url.is_empty(),
        "--redis-url (or CHECKGATE_TEST_REDIS_URL) is required"
    );
    a
}

// ── Statistics ──────────────────────────────────────────────────────────────

fn summary(mut samples: Vec<f64>) -> Value {
    if samples.is_empty() {
        return json!({ "samples": 0 });
    }
    samples.sort_by(f64::total_cmp);
    let pct = |p: f64| samples[(((samples.len() - 1) as f64) * p).round() as usize];
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    json!({
        "samples": samples.len(),
        "p50_ms": round(pct(0.50)), "p95_ms": round(pct(0.95)), "p99_ms": round(pct(0.99)),
        "max_ms": round(*samples.last().unwrap()), "mean_ms": round(mean),
    })
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

// ── Server processes ────────────────────────────────────────────────────────

struct Instance {
    child: Child,
    base: String,
    metrics: String,
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// (user+system CPU seconds, resident set size in bytes) of a process.
fn proc_usage(pid: u32) -> (f64, u64) {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    // Fields after the parenthesised command name; utime and stime are 14 and 15.
    let after = stat.rsplit(')').next().unwrap_or("");
    let fields: Vec<&str> = after.split_whitespace().collect();
    let ticks = |i: usize| {
        fields
            .get(i)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    // Linux reports in USER_HZ, which is 100 on every mainstream configuration.
    let cpu = (ticks(11) + ticks(12)) / 100.0;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    let rss_kb = status
        .lines()
        .find(|l| l.starts_with("VmRSS:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    (cpu, rss_kb * 1024)
}

async fn fresh_database(admin_url: &str, name: &str) -> String {
    let pool = sqlx::PgPool::connect(admin_url).await.unwrap();
    sqlx::query(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(&format!("CREATE DATABASE \"{name}\""))
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let mut url = reqwest::Url::parse(admin_url).unwrap();
    url.set_path(&format!("/{name}"));
    url.to_string()
}

async fn spawn(a: &Args, db_url: &str, channel: &str) -> Instance {
    let port = free_port();
    let mport = free_port();
    let dir = std::env::temp_dir().join(format!("checkgate-loadgen-{port}"));
    std::fs::create_dir_all(&dir).unwrap();
    let child = Command::new(&a.server_bin)
        .env("DATABASE_URL", db_url)
        .env("REDIS_URL", &a.redis_url)
        .env("REDIS_CHANNEL", channel)
        .env("PORT", port.to_string())
        .env("METRICS_ADDR", format!("127.0.0.1:{mport}"))
        .env(
            "SESSION_SECRET",
            "loadgen-secret-0123456789-0123456789-0123456789",
        )
        .env("SETUP_TOKEN", "loadgen-setup-token-0123456789")
        .env("COOKIE_SECURE", "false")
        .env("DB_MAX_CONNECTIONS", a.db_max_connections.to_string())
        .env("SSE_MAX_CONNECTIONS", "0")
        .env("PUBLIC_DIR", &dir)
        .env("RUST_LOG", "warn")
        .env("RATE_LIMIT_AUTH_PER_MINUTE", "0")
        .env("RATE_LIMIT_API_PER_MINUTE", "0")
        .env("RATE_LIMIT_SDK_PER_MINUTE", "0")
        .env("RATE_LIMIT_SSE_PER_MINUTE", "0")
        .env("RATE_LIMIT_INGEST_PER_MINUTE", "0")
        .current_dir(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn server");
    let inst = Instance {
        child,
        base: format!("http://127.0.0.1:{port}"),
        metrics: format!("http://127.0.0.1:{mport}/metrics"),
    };
    for _ in 0..240 {
        if reqwest::get(format!("{}/health/ready", inst.base))
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            return inst;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("server did not become ready");
}

/// Mean of a histogram from the metrics text: `<name>_sum / <name>_count`
/// over every series whose label set contains `filter`.
fn histogram_mean_ms(text: &str, name: &str, filter: &str) -> Option<f64> {
    let total = |suffix: &str| -> f64 {
        text.lines()
            .filter(|l| l.starts_with(&format!("{name}{suffix}")) && l.contains(filter))
            .filter_map(|l| l.rsplit(' ').next()?.parse::<f64>().ok())
            .sum()
    };
    let count = total("_count");
    (count > 0.0).then(|| round(total("_sum") / count * 1000.0))
}

fn gauge(text: &str, name: &str) -> Option<f64> {
    text.lines()
        .find(|l| l.starts_with(name) && l[name.len()..].starts_with(' '))
        .and_then(|l| l.rsplit(' ').next()?.parse().ok())
}

// ── SSE connections ─────────────────────────────────────────────────────────

/// Receive time of every change event, keyed by revision, per connection.
type Arrivals = Arc<Mutex<Vec<HashMap<i64, Instant>>>>;

async fn open_stream(
    client: reqwest::Client,
    base: String,
    key: String,
    slot: usize,
    arrivals: Arrivals,
    ready: tokio::sync::mpsc::UnboundedSender<Result<Duration, String>>,
) {
    let started = Instant::now();
    let resp = match client
        .get(format!("{base}/stream"))
        .bearer_auth(&key)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            let _ = ready.send(Err(format!("HTTP {}", r.status())));
            return;
        }
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    let mut body = resp.bytes_stream();
    let mut buf = String::new();
    let mut event = String::new();
    let mut is_ready = false;
    while let Some(Ok(chunk)) = body.next().await {
        let now = Instant::now();
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim_end_matches('\r').to_string();
            buf.drain(..=nl);
            if let Some(v) = line.strip_prefix("event:") {
                event = v.trim().to_string();
            } else if let Some(data) = line.strip_prefix("data:") {
                if event == "ready" && !is_ready {
                    is_ready = true;
                    let _ = ready.send(Ok(started.elapsed()));
                } else if is_ready
                    && event == "update"
                    && let Ok(v) = serde_json::from_str::<Value>(data.trim())
                    && let Some(rev) = v["revision"].as_i64()
                {
                    arrivals.lock().unwrap()[slot].insert(rev, now);
                }
            }
        }
    }
    if !is_ready {
        let _ = ready.send(Err("closed before ready".into()));
    }
}

// ── Scenarios ───────────────────────────────────────────────────────────────

struct Ctx {
    admin: reqwest::Client,
    sdk_key: String,
    env: String,
    instances: Vec<Instance>,
}

async fn rest_latency(ctx: &Ctx, n: usize) -> Value {
    let base = &ctx.instances[0].base;
    let mut list = Vec::new();
    let mut patch = Vec::new();
    for i in 0..n {
        let t = Instant::now();
        let r = ctx
            .admin
            .get(format!(
                "{base}/api/environments/{}/flags?limit=100",
                ctx.env
            ))
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success());
        let _ = r.bytes().await;
        list.push(ms(t.elapsed()));
        let t = Instant::now();
        let r = ctx
            .admin
            .patch(format!(
                "{base}/api/environments/{}/flags/flag-{}",
                ctx.env,
                i % 10
            ))
            .header("X-Checkgate-Request", "true")
            .json(&json!({ "is_enabled": i % 2 == 0 }))
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success());
        patch.push(ms(t.elapsed()));
    }
    json!({ "list_flags": summary(list), "patch_flag": summary(patch) })
}

async fn snapshot_throughput(ctx: &Ctx, secs: u64, concurrency: usize) -> Value {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let client = reqwest::Client::new();
    let mut tasks = Vec::new();
    for i in 0..concurrency {
        let (client, key) = (client.clone(), ctx.sdk_key.clone());
        let base = ctx.instances[i % ctx.instances.len()].base.clone();
        tasks.push(tokio::spawn(async move {
            let mut lat = Vec::new();
            let mut errors = 0u64;
            while Instant::now() < deadline {
                let t = Instant::now();
                match client
                    .get(format!("{base}/flags/snapshot"))
                    .bearer_auth(&key)
                    .send()
                    .await
                {
                    Ok(r) if r.status().is_success() => {
                        let _ = r.bytes().await;
                        lat.push(ms(t.elapsed()));
                    }
                    _ => errors += 1,
                }
            }
            (lat, errors)
        }));
    }
    let mut all = Vec::new();
    let mut errors = 0;
    for t in tasks {
        let (l, e) = t.await.unwrap();
        all.extend(l);
        errors += e;
    }
    let count = all.len();
    json!({
        "concurrency": concurrency, "duration_s": secs, "requests": count, "errors": errors,
        "requests_per_s": round(count as f64 / secs as f64), "latency": summary(all),
    })
}

async fn connection_profile(ctx: &Ctx, n: usize, updates: usize) -> Value {
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()
        .unwrap();
    let arrivals: Arrivals = Arc::new(Mutex::new(vec![HashMap::new(); n]));
    let (ready_tx, mut ready_rx) = tokio::sync::mpsc::unbounded_channel();
    let before: Vec<(f64, u64)> = ctx
        .instances
        .iter()
        .map(|i| proc_usage(i.child.id()))
        .collect();

    // Ramp-up: at most 200 handshakes in flight, like a reconnect wave.
    let gate = Arc::new(Semaphore::new(200));
    let ramp = Instant::now();
    let mut streams = Vec::new();
    for slot in 0..n {
        let permit = Arc::clone(&gate).acquire_owned().await.unwrap();
        let (client, key, arrivals, tx) = (
            client.clone(),
            ctx.sdk_key.clone(),
            Arc::clone(&arrivals),
            ready_tx.clone(),
        );
        let base = ctx.instances[slot % ctx.instances.len()].base.clone();
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        streams.push(tokio::spawn(open_stream(
            client, base, key, slot, arrivals, inner_tx,
        )));
        tokio::spawn(async move {
            if let Some(r) = inner_rx.recv().await {
                let _ = tx.send(r);
            }
            drop(permit);
        });
    }
    let mut connect = Vec::new();
    let mut failures: HashMap<String, usize> = HashMap::new();
    for _ in 0..n {
        match tokio::time::timeout(Duration::from_secs(120), ready_rx.recv()).await {
            Ok(Some(Ok(d))) => connect.push(ms(d)),
            Ok(Some(Err(e))) => *failures.entry(e).or_default() += 1,
            _ => *failures.entry("timeout".into()).or_default() += 1,
        }
    }
    let ramp_s = ramp.elapsed().as_secs_f64();

    // Hold idle for 5 s and sample resources.
    tokio::time::sleep(Duration::from_secs(5)).await;
    let held: Vec<Value> = ctx
        .instances
        .iter()
        .zip(&before)
        .map(|(inst, (cpu0, _))| {
            let (cpu, rss) = proc_usage(inst.child.id());
            json!({ "rss_bytes": rss, "cpu_seconds_since_ramp_start": round(cpu - cpu0) })
        })
        .collect();

    // Propagation: updates through instance 0, one every 50 ms.
    let cpu_before: Vec<f64> = ctx
        .instances
        .iter()
        .map(|i| proc_usage(i.child.id()).0)
        .collect();
    let prop_start = Instant::now();
    let mut sends: Vec<(Instant, Instant)> = Vec::new();
    for u in 0..updates {
        let t = Instant::now();
        let r = ctx
            .admin
            .patch(format!(
                "{}/api/environments/{}/flags/hot",
                ctx.instances[0].base, ctx.env
            ))
            .header("X-Checkgate-Request", "true")
            .json(&json!({ "is_enabled": u % 2 == 0 }))
            .send()
            .await
            .unwrap();
        let done = Instant::now();
        assert!(r.status().is_success(), "patch hot flag");
        sends.push((t, done));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    let prop_wall = prop_start.elapsed().as_secs_f64();
    let cpu_prop: Vec<f64> = ctx
        .instances
        .iter()
        .zip(&cpu_before)
        .map(|(i, c0)| round((proc_usage(i.child.id()).0 - c0) / prop_wall))
        .collect();

    // Match arrivals to sends: revisions increase with each send, so the k-th
    // smallest revision seen belongs to the k-th send.
    let arrivals = arrivals.lock().unwrap().clone();
    let mut revisions: Vec<i64> = arrivals.iter().flat_map(|m| m.keys().copied()).collect();
    revisions.sort_unstable();
    revisions.dedup();
    let mut visible = Vec::new();
    let mut after_response = Vec::new();
    let mut fanout = Vec::new();
    let mut missing = 0usize;
    for (k, (sent, responded)) in sends.iter().enumerate() {
        let Some(rev) = revisions.get(k) else {
            missing += n;
            continue;
        };
        let times: Vec<Instant> = arrivals
            .iter()
            .filter_map(|m| m.get(rev).copied())
            .collect();
        missing += n - times.len();
        if let (Some(first), Some(last)) = (times.iter().min(), times.iter().max()) {
            fanout.push(ms(*last - *first));
        }
        for t in times {
            visible.push(ms(t - *sent));
            after_response.push(ms(t.saturating_duration_since(*responded)));
        }
    }
    for s in streams {
        s.abort();
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    json!({
        "connections": n,
        "instances": ctx.instances.len(),
        "ramp": { "seconds": round(ramp_s), "handshakes_in_flight": 200, "failures": failures,
                   "time_to_ready": summary(connect) },
        "held_idle": held,
        "propagation": {
            "updates": updates, "interval_ms": 50,
            "write_to_visible": summary(visible),
            "visible_after_http_response": summary(after_response),
            "fanout_spread_first_to_last": summary(fanout),
            "missed_deliveries": missing,
            "server_cpu_cores_during_propagation": cpu_prop,
        },
    })
}

#[tokio::main]
async fn main() {
    let a = args();
    let db_url = fresh_database(&a.database_url, "checkgate_loadgen").await;
    let channel = format!("checkgate_loadgen_{}", std::process::id());
    let mut instances = Vec::new();
    for _ in 0..a.instances {
        instances.push(spawn(&a, &db_url, &channel).await);
    }
    let base = instances[0].base.clone();
    let setup = reqwest::Client::new();
    let key: Value = setup
        .get(format!("{base}/api/setup/key"))
        .header("X-Checkgate-Setup-Token", "loadgen-setup-token-0123456789")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let admin = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    let r = admin
        .post(format!("{base}/api/setup/complete"))
        .header("X-Checkgate-Request", "true")
        .header("X-Checkgate-Setup-Token", "loadgen-setup-token-0123456789")
        .json(
            &json!({ "workspace_name": "Load", "project_name": "Load", "name": "Admin",
                       "email": "admin@load.test", "password": "load-password-1" }),
        )
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "setup");
    let env = key["environment_id"].as_str().unwrap().to_string();
    let ctx = Ctx {
        admin,
        sdk_key: key["key"].as_str().unwrap().to_string(),
        env,
        instances,
    };

    // Representative flag set: mostly booleans, some targeting and rollouts.
    for i in 0..a.flags {
        let body = match i % 4 {
            0 => {
                json!({ "key": format!("flag-{i}"), "is_enabled": true, "flag_type": "boolean", "rules": [] })
            }
            1 => {
                json!({ "key": format!("flag-{i}"), "is_enabled": true, "rollout_percentage": 25, "flag_type": "boolean", "rules": [] })
            }
            2 => json!({ "key": format!("flag-{i}"), "is_enabled": true, "flag_type": "boolean",
                         "rules": [{ "attribute": "email", "operator": "ends_with", "values": ["@acme.test"] }] }),
            _ => json!({ "key": format!("flag-{i}"), "is_enabled": true, "flag_type": "string",
                         "default_value": "a", "variants": [{ "weight": 50, "value": "a" }, { "weight": 50, "value": "b" }], "rules": [] }),
        };
        let r = ctx
            .admin
            .post(format!("{base}/api/environments/{}/flags", ctx.env))
            .header("X-Checkgate-Request", "true")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success(), "create flag");
    }
    let r = ctx
        .admin
        .post(format!("{base}/api/environments/{}/flags", ctx.env))
        .header("X-Checkgate-Request", "true")
        .json(&json!({ "key": "hot", "is_enabled": false, "flag_type": "boolean", "rules": [] }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());

    eprintln!("rest latency…");
    let rest = rest_latency(&ctx, 200).await;
    eprintln!("snapshot throughput…");
    let snapshot = snapshot_throughput(&ctx, a.snapshot_secs, a.snapshot_concurrency).await;
    let mut profiles = Vec::new();
    for &n in &a.connections {
        eprintln!("{n} connections…");
        profiles.push(connection_profile(&ctx, n, a.updates).await);
    }
    let mut server_side = Vec::new();
    for inst in &ctx.instances {
        let text = reqwest::get(&inst.metrics)
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        server_side.push(json!({
            "patch_handling_mean_ms": histogram_mean_ms(&text, "checkgate_http_request_duration_seconds", "method=\"PATCH\""),
            "redis_publish_mean_ms": histogram_mean_ms(&text, "checkgate_flag_publish_duration_seconds", ""),
            "sse_bootstrap_mean_ms": histogram_mean_ms(&text, "checkgate_sse_bootstrap_duration_seconds", ""),
            "snapshot_query_mean_ms": histogram_mean_ms(&text, "checkgate_snapshot_query_duration_seconds", ""),
            "db_pool_connections": gauge(&text, "checkgate_db_pool_connections"),
            "db_pool_max_connections": gauge(&text, "checkgate_db_pool_max_connections"),
            "sse_rejections": text.lines().filter(|l| l.starts_with("checkgate_sse_rejections_total")).map(str::to_string).collect::<Vec<_>>(),
            "sse_lagged_messages": gauge(&text, "checkgate_sse_lagged_messages_total"),
            "publish_failures": gauge(&text, "checkgate_flag_update_publish_failures_total"),
        }));
    }
    let result = json!({
        "generated_by": "server/examples/loadgen.rs",
        "config": { "instances": a.instances, "flags": a.flags + 1, "db_max_connections": a.db_max_connections,
                    "updates_per_profile": a.updates, "server_bin": a.server_bin },
        "rest_latency": rest,
        "snapshot": snapshot,
        "connection_profiles": profiles,
        "server_metrics": server_side,
    });
    let text = serde_json::to_string_pretty(&result).unwrap();
    println!("{text}");
    if let Some(out) = a.out {
        std::fs::write(out, text).unwrap();
    }
}
