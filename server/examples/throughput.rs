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
//! Closed-loop throughput of the SDK snapshot read path (`GET /flags/snapshot`)
//! against an already running, resource-limited server.
//!
//! Unlike `loadgen.rs`, this does not start the server: it is meant to run in
//! its own container next to a server container that has a CPU and memory
//! limit (see `bench/public/compose.yml`). The generator shares the server's
//! PID namespace (`pid: service:server`) so it can read the server's CPU time
//! and resident memory from `/proc/<pid>`, once per second, during each
//! measurement window. It also records its own CPU so a saturated generator is
//! visible in the results.
//!
//! For each concurrency level: `--warmup-secs` of load that is not recorded,
//! then `--secs` of measurement. A request counts when it completes inside
//! the window (throughput = completions per second); one that started during
//! the warm-up is still steady-state. Requiring a request to also *start*
//! inside the window would drop both edges and undercount a closed loop by
//! about mean latency / window. Every response must be 200 with the same body
//! length as the first validated snapshot; anything else is an error.
//!
//!     throughput --url http://server:3000 --session /work/session.json \
//!         --concurrency 10,100,500,1000 --warmup-secs 10 --secs 60 \
//!         --server-pid 1 --out /results/throughput.json

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

struct Args {
    url: String,
    session: String,
    concurrency: Vec<usize>,
    warmup_secs: u64,
    secs: u64,
    pause_secs: u64,
    server_pid: u32,
    out: Option<String>,
}

fn args() -> Args {
    let mut a = Args {
        url: "http://server:3000".into(),
        session: "/work/session.json".into(),
        concurrency: vec![10, 100, 500, 1000],
        warmup_secs: 10,
        secs: 60,
        pause_secs: 5,
        server_pid: 1,
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut v = || it.next().unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--url" => a.url = v(),
            "--session" => a.session = v(),
            "--concurrency" => {
                a.concurrency = v().split(',').map(|n| n.trim().parse().unwrap()).collect()
            }
            "--warmup-secs" => a.warmup_secs = v().parse().unwrap(),
            "--secs" => a.secs = v().parse().unwrap(),
            "--pause-secs" => a.pause_secs = v().parse().unwrap(),
            "--server-pid" => a.server_pid = v().parse().unwrap(),
            "--out" => a.out = Some(v()),
            other => panic!("unknown argument {other}"),
        }
    }
    assert!(a.secs > 0, "--secs must be positive");
    a
}

// ── Statistics ──────────────────────────────────────────────────────────────

/// Nearest-rank percentiles of latencies held in microseconds.
fn latency_summary(mut us: Vec<u32>) -> Value {
    if us.is_empty() {
        return json!({ "samples": 0 });
    }
    us.sort_unstable();
    let pct = |p: f64| f64::from(us[(((us.len() - 1) as f64) * p).round() as usize]) / 1000.0;
    let mean = us.iter().map(|&v| f64::from(v)).sum::<f64>() / us.len() as f64 / 1000.0;
    json!({
        "samples": us.len(),
        "p50_ms": round3(pct(0.50)), "p90_ms": round3(pct(0.90)), "p95_ms": round3(pct(0.95)),
        "p99_ms": round3(pct(0.99)), "p999_ms": round3(pct(0.999)),
        "max_ms": round3(f64::from(*us.last().unwrap()) / 1000.0), "mean_ms": round3(mean),
    })
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    s.get(s.len() / 2).copied().unwrap_or(0.0)
}

fn max(v: &[f64]) -> f64 {
    v.iter().copied().fold(0.0, f64::max)
}

// ── /proc sampling ──────────────────────────────────────────────────────────

/// Linux reports CPU time in USER_HZ, which is 100 on every mainstream kernel.
const USER_HZ: f64 = 100.0;

/// (user+system CPU seconds, VmRSS bytes, VmHWM bytes) of `pid`, or `None`
/// when `/proc/<pid>` is not readable (not in a shared PID namespace).
fn proc_usage(pid: &str) -> Option<(f64, u64, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Fields after the parenthesised command name; utime and stime are 14 and 15.
    let after = stat.rsplit(')').next()?;
    let fields: Vec<&str> = after.split_whitespace().collect();
    let ticks = |i: usize| fields.get(i).and_then(|v| v.parse::<f64>().ok());
    let cpu = (ticks(11)? + ticks(12)?) / USER_HZ;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let kb = |key: &str| {
        status
            .lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v * 1024)
    };
    Some((cpu, kb("VmRSS:")?, kb("VmHWM:")?))
}

#[derive(Default)]
struct Samples {
    server_cores: Vec<f64>,
    server_rss: Vec<f64>,
    generator_cores: Vec<f64>,
}

/// Samples server and generator CPU (cores, per 1 s interval) and server RSS
/// until `stop` is set.
async fn sample(server_pid: String, stop: Arc<AtomicBool>) -> Samples {
    let mut out = Samples::default();
    let mut prev_server = proc_usage(&server_pid);
    let mut prev_self = proc_usage("self");
    let mut prev_t = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.tick().await;
    while !stop.load(Ordering::Acquire) {
        tick.tick().await;
        let now = Instant::now();
        let dt = (now - prev_t).as_secs_f64();
        prev_t = now;
        let server = proc_usage(&server_pid);
        if let (Some((c0, _, _)), Some((c1, rss, _))) = (prev_server, server) {
            out.server_cores.push((c1 - c0) / dt);
            out.server_rss.push(rss as f64);
        }
        let me = proc_usage("self");
        if let (Some((c0, _, _)), Some((c1, _, _))) = (prev_self, me) {
            out.generator_cores.push((c1 - c0) / dt);
        }
        prev_server = server;
        prev_self = me;
    }
    out
}

// ── Load ────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct WorkerResult {
    latencies_us: Vec<u32>,
    errors: BTreeMap<String, u64>,
    bytes: u64,
}

async fn worker(
    client: reqwest::Client,
    url: String,
    key: String,
    expected_len: usize,
    measure_from: Instant,
    measure_to: Instant,
) -> WorkerResult {
    let mut r = WorkerResult::default();
    loop {
        let start = Instant::now();
        if start >= measure_to {
            break;
        }
        let outcome = match client.get(&url).bearer_auth(&key).send().await {
            Ok(resp) if resp.status().is_success() => match resp.bytes().await {
                Ok(body) if body.len() == expected_len => Ok(body.len()),
                Ok(body) => Err(format!("body length {}", body.len())),
                Err(e) => Err(format!("body: {}", error_kind(&e))),
            },
            Ok(resp) => Err(format!("HTTP {}", resp.status().as_u16())),
            Err(e) => Err(error_kind(&e)),
        };
        let end = Instant::now();
        // Count requests that complete inside the window.
        if end < measure_from || end > measure_to {
            continue;
        }
        match outcome {
            Ok(len) => {
                let us = u32::try_from((end - start).as_micros()).unwrap_or(u32::MAX);
                r.latencies_us.push(us);
                r.bytes += len as u64;
            }
            Err(kind) => *r.errors.entry(kind).or_default() += 1,
        }
    }
    r
}

fn error_kind(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timeout".into()
    } else if e.is_connect() {
        "connect".into()
    } else {
        "transport".into()
    }
}

async fn level(a: &Args, key: &str, expected_len: usize, concurrency: usize) -> Value {
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(concurrency)
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let url = format!("{}/flags/snapshot", a.url);
    let started = Instant::now();
    let measure_from = started + Duration::from_secs(a.warmup_secs);
    let measure_to = measure_from + Duration::from_secs(a.secs);
    let tasks: Vec<_> = (0..concurrency)
        .map(|_| {
            tokio::spawn(worker(
                client.clone(),
                url.clone(),
                key.to_string(),
                expected_len,
                measure_from,
                measure_to,
            ))
        })
        .collect();

    tokio::time::sleep_until(measure_from.into()).await;
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = tokio::spawn(sample(a.server_pid.to_string(), Arc::clone(&stop)));
    tokio::time::sleep_until(measure_to.into()).await;
    stop.store(true, Ordering::Release);
    let samples = sampler.await.unwrap();

    let mut latencies = Vec::new();
    let mut errors: BTreeMap<String, u64> = BTreeMap::new();
    let mut bytes = 0u64;
    for t in tasks {
        let r = t.await.unwrap();
        latencies.extend(r.latencies_us);
        for (k, v) in r.errors {
            *errors.entry(k).or_default() += v;
        }
        bytes += r.bytes;
    }
    let ok = latencies.len() as u64;
    let failed: u64 = errors.values().sum();
    let window = a.secs as f64;
    let rps = ok as f64 / window;
    let server_cpu_mean = mean(&samples.server_cores);
    let rss_median = median(&samples.server_rss);
    let mib = |b: f64| round3(b / (1024.0 * 1024.0));
    json!({
        "concurrency": concurrency,
        "warmup_s": a.warmup_secs,
        "window_s": a.secs,
        "successful_requests": ok,
        "failed_requests": failed,
        "errors": errors,
        "error_rate": if ok + failed == 0 { 0.0 } else { failed as f64 / (ok + failed) as f64 },
        "successful_rps": round3(rps),
        "response_bytes": expected_len,
        "throughput_mib_per_s": mib(bytes as f64 / window),
        "latency": latency_summary(latencies),
        "server": {
            "cpu_cores_mean": round3(server_cpu_mean),
            "cpu_cores_peak_1s": round3(max(&samples.server_cores)),
            "rss_mib_median": mib(rss_median),
            "rss_mib_peak_1s": mib(max(&samples.server_rss)),
            "cpu_samples": samples.server_cores.len(),
            // Successful requests per CPU-second the server actually used.
            "requests_per_cpu_second": if server_cpu_mean > 0.0 { round3(rps / server_cpu_mean) } else { 0.0 },
        },
        "generator": {
            "cpu_cores_mean": round3(mean(&samples.generator_cores)),
            "cpu_cores_peak_1s": round3(max(&samples.generator_cores)),
        },
    })
}

#[tokio::main]
async fn main() {
    let a = args();
    let session: Value =
        serde_json::from_str(&std::fs::read_to_string(&a.session).expect("read --session"))
            .expect("parse --session");
    let key = session["sdk_key"]
        .as_str()
        .expect("session.sdk_key")
        .to_string();
    let expected_flags = session["flags"].as_u64().expect("session.flags");

    // Validate the payload once: the right flag count, parsed as JSON. During
    // load only the status and the body length are checked.
    let probe = reqwest::Client::new()
        .get(format!("{}/flags/snapshot", a.url))
        .bearer_auth(&key)
        .send()
        .await
        .expect("probe snapshot");
    assert!(
        probe.status().is_success(),
        "probe: HTTP {}",
        probe.status()
    );
    let body = probe.bytes().await.expect("probe body");
    let flags: Vec<Value> = serde_json::from_slice(&body).expect("snapshot is a JSON array");
    assert_eq!(flags.len() as u64, expected_flags, "snapshot flag count");
    let expected_len = body.len();

    let server_proc_visible = proc_usage(&a.server_pid.to_string()).is_some();
    if !server_proc_visible {
        eprintln!(
            "warning: /proc/{} is not readable — server CPU/RSS will be missing",
            a.server_pid
        );
    }
    let (_, rss_before, _) = proc_usage(&a.server_pid.to_string()).unwrap_or_default();

    let mut levels = Vec::new();
    for (i, &c) in a.concurrency.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(Duration::from_secs(a.pause_secs)).await;
        }
        eprintln!("concurrency {c}…");
        let r = level(&a, &key, expected_len, c).await;
        eprintln!(
            "  {} req/s, p99 {} ms, errors {}",
            r["successful_rps"], r["latency"]["p99_ms"], r["failed_requests"]
        );
        levels.push(r);
    }
    let (_, rss_after, hwm) = proc_usage(&a.server_pid.to_string()).unwrap_or_default();
    let mib = |b: u64| round3(b as f64 / (1024.0 * 1024.0));
    let result = json!({
        "generated_by": "server/examples/throughput.rs",
        "endpoint": "GET /flags/snapshot",
        "flags_in_snapshot": expected_flags,
        "response_bytes": expected_len,
        "server_proc_visible": server_proc_visible,
        "server_rss_mib_before": mib(rss_before),
        "server_rss_mib_after": mib(rss_after),
        "server_rss_mib_high_water_mark": mib(hwm),
        "levels": levels,
    });
    let text = serde_json::to_string_pretty(&result).unwrap();
    println!("{text}");
    if let Some(out) = a.out {
        std::fs::write(out, text).expect("write --out");
    }
}
