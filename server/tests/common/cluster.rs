//! Several independently running server processes sharing one PostgreSQL
//! database and one Redis server — each instance reaching Redis through its
//! own [`ChaosProxy`], so faults can target a single instance.
//!
//! The Redis channel name is unique per cluster (`REDIS_CHANNEL`), so
//! clusters of tests running in parallel never see each other's events.
//! When a test panics, every instance's log tail is printed, and with
//! `CHECKGATE_TEST_ARTIFACTS=<dir>` the full logs are copied there.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use super::chaos::ChaosProxy;
use super::sdk::SimSdk;
use super::{Server, TestEnv, complete_setup, free_port, request};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct Instance {
    pub server: Option<Server>,
    pub redis: ChaosProxy,
    pub redis_url: String,
    pub port: u16,
    pub dir: PathBuf,
}

pub struct Cluster {
    pub name: String,
    pub db_url: String,
    pub channel: String,
    pub instances: Vec<Instance>,
    pub env: Vec<(String, Option<String>)>,
    pub admin: reqwest::Client,
    pub sdk_key: String,
    pub env_id: String,
    pub pool: sqlx::PgPool,
}

/// Settings every cluster instance gets: no rate limits (tests open many
/// streams from one address), and a fast reconciliation cycle so recovery
/// tests finish in seconds. Tests override any of them.
pub const FAST_SYNC: &[(&str, Option<&str>)] = &[
    ("RATE_LIMIT_AUTH_PER_MINUTE", Some("0")),
    ("RATE_LIMIT_API_PER_MINUTE", Some("0")),
    ("RATE_LIMIT_SDK_PER_MINUTE", Some("0")),
    ("RATE_LIMIT_SSE_PER_MINUTE", Some("0")),
    ("RATE_LIMIT_INGEST_PER_MINUTE", Some("0")),
    ("SYNC_RECONCILE_INTERVAL_SECS", Some("1")),
    ("SYNC_RECONCILE_GRACE_SECS", Some("2")),
    ("REDIS_PING_INTERVAL_SECS", Some("1")),
    ("SSE_RESYNC_SPREAD_MS", Some("200")),
];

impl Cluster {
    /// Starts `n` instances against a fresh database named after the test.
    pub async fn start(
        test_env: &TestEnv,
        name: &str,
        n: usize,
        extra_env: &[(&str, Option<&str>)],
    ) -> Self {
        let db_url = test_env.fresh_database(name).await;
        let channel = format!("checkgate_test_{name}_{}", std::process::id());
        let mut env: Vec<(String, Option<String>)> = FAST_SYNC
            .iter()
            .map(|(k, v)| (k.to_string(), v.map(str::to_string)))
            .collect();
        env.push(("REDIS_CHANNEL".into(), Some(channel.clone())));
        for (k, v) in extra_env {
            env.retain(|(key, _)| key != k);
            env.push((k.to_string(), v.map(str::to_string)));
        }

        let mut instances = Vec::new();
        for _ in 0..n {
            let (redis, redis_url) = ChaosProxy::for_url(&test_env.redis_url).await;
            let port = free_port();
            let dir = std::env::temp_dir().join(format!("checkgate-test-{name}-{port}"));
            let _ = std::fs::remove_dir_all(&dir);
            instances.push(Instance {
                server: None,
                redis,
                redis_url,
                port,
                dir,
            });
        }
        let mut cluster = Cluster {
            name: name.to_string(),
            pool: sqlx::PgPool::connect(&db_url).await.unwrap(),
            db_url,
            channel,
            instances,
            env,
            admin: reqwest::Client::new(),
            sdk_key: String::new(),
            env_id: String::new(),
        };
        // Start the first instance alone so it runs the migrations, then the rest.
        cluster.start_instance(0).await;
        for i in 1..n {
            cluster.start_instance(i).await;
        }
        let (admin, sdk_key, env_id) =
            complete_setup(cluster.instances[0].server.as_ref().unwrap()).await;
        cluster.admin = admin;
        cluster.sdk_key = sdk_key;
        cluster.env_id = env_id;
        cluster
    }

    fn env_pairs(&self) -> Vec<(&str, Option<&str>)> {
        self.env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_deref()))
            .collect()
    }

    /// Starts instance `i` on its port. On first start, a port taken by
    /// another process in the meantime is replaced by a fresh one; a restart
    /// keeps its port, since clients point at it.
    pub async fn start_instance(&mut self, i: usize) {
        let first_start = !self.instances[i].dir.join("server.log").exists();
        for _ in 0..5 {
            let env = self.env_pairs();
            let inst = &self.instances[i];
            if let Some(server) =
                Server::try_spawn_in(&inst.dir, inst.port, &self.db_url, &inst.redis_url, &env)
                    .await
            {
                self.instances[i].server = Some(server);
                return;
            }
            assert!(
                first_start,
                "restarted instance exited during startup:\n{}",
                self.logs()[i].1
            );
            let port = free_port();
            let dir = std::env::temp_dir().join(format!("checkgate-test-{}-{port}", self.name));
            let _ = std::fs::remove_dir_all(&dir);
            self.instances[i].port = port;
            self.instances[i].dir = dir;
        }
        panic!("instance {i} could not start on a free port");
    }

    /// SIGKILL: no shutdown handling runs, like a crashed or OOM-killed pod.
    pub fn kill(&mut self, i: usize) {
        self.instances[i].server.take();
    }

    pub fn server(&self, i: usize) -> &Server {
        self.instances[i].server.as_ref().expect("instance running")
    }

    pub fn server_mut(&mut self, i: usize) -> &mut Server {
        self.instances[i].server.as_mut().expect("instance running")
    }

    pub fn base(&self, i: usize) -> String {
        format!("http://127.0.0.1:{}", self.instances[i].port)
    }

    pub fn redis(&self, i: usize) -> &ChaosProxy {
        &self.instances[i].redis
    }

    pub fn sdk(&self, name: &str, i: usize) -> SimSdk {
        SimSdk::start(name, &self.base(i), &self.sdk_key)
    }

    fn flags_url(&self, i: usize, env: &str) -> String {
        format!("{}/api/environments/{env}/flags", self.base(i))
    }

    pub async fn create_flag(&self, i: usize, key: &str, enabled: bool) -> Value {
        self.create_flag_body(i, &self.env_id, flag_body(key, enabled))
            .await
    }

    pub async fn create_flag_body(&self, i: usize, env: &str, body: Value) -> Value {
        request(
            &self.admin,
            "POST",
            &self.flags_url(i, env),
            Some(body),
            200,
        )
        .await
    }

    pub async fn set_enabled(&self, i: usize, key: &str, enabled: bool) -> Value {
        let url = format!("{}/{key}", self.flags_url(i, &self.env_id));
        request(
            &self.admin,
            "PATCH",
            &url,
            Some(json!({ "is_enabled": enabled })),
            200,
        )
        .await
    }

    /// PATCH without asserting the status — for races where a concurrent
    /// delete may win (404).
    pub async fn try_set_enabled(&self, i: usize, key: &str, enabled: bool) -> u16 {
        let url = format!("{}/{key}", self.flags_url(i, &self.env_id));
        self.admin
            .patch(url)
            .header("X-Checkgate-Request", "true")
            .json(&json!({ "is_enabled": enabled }))
            .send()
            .await
            .map(|r| r.status().as_u16())
            .unwrap_or(0)
    }

    pub async fn try_delete(&self, i: usize, key: &str) -> u16 {
        let url = format!("{}/{key}", self.flags_url(i, &self.env_id));
        self.admin
            .delete(url)
            .header("X-Checkgate-Request", "true")
            .send()
            .await
            .map(|r| r.status().as_u16())
            .unwrap_or(0)
    }

    pub async fn try_create(&self, i: usize, key: &str, enabled: bool) -> u16 {
        self.admin
            .post(self.flags_url(i, &self.env_id))
            .header("X-Checkgate-Request", "true")
            .json(&flag_body(key, enabled))
            .send()
            .await
            .map(|r| r.status().as_u16())
            .unwrap_or(0)
    }

    /// The committed state of an environment: key → (revision, is_enabled).
    pub async fn authoritative(&self, env: &str) -> HashMap<String, (i64, bool)> {
        sqlx::query_as::<_, (String, i64, Option<bool>)>(
            "SELECT key, revision, (data->>'is_enabled')::bool FROM flags \
             WHERE environment_id = $1::uuid",
        )
        .bind(env)
        .fetch_all(&self.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|(k, r, e)| (k, (r, e.unwrap_or(false))))
        .collect()
    }

    /// Waits until every client holds exactly the committed state of `env`
    /// and returns how long that took. Panics with the differences (and the
    /// instance logs) after `within`.
    pub async fn wait_converged(&self, sdks: &[&SimSdk], env: &str, within: Duration) -> Duration {
        let start = Instant::now();
        loop {
            let truth = self.authoritative(env).await;
            let diffs: Vec<String> = sdks
                .iter()
                .filter_map(|sdk| divergence(sdk, &truth).map(|d| format!("{}: {d}", sdk.name)))
                .collect();
            if diffs.is_empty() {
                return start.elapsed();
            }
            if start.elapsed() > within {
                panic!(
                    "clients did not converge to the committed state within {within:?}:\n{}",
                    diffs.join("\n")
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Records a measured recovery window: printed, and appended to
    /// `$CHECKGATE_TEST_ARTIFACTS/recovery.jsonl` when set.
    pub fn record(&self, scenario: &str, elapsed: Duration) {
        let line = json!({
            "test": self.name,
            "scenario": scenario,
            "recovery_ms": u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        });
        eprintln!("RECOVERY {line}");
        if let Ok(dir) = std::env::var("CHECKGATE_TEST_ARTIFACTS") {
            use std::io::Write as _;
            let _ = std::fs::create_dir_all(&dir);
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(PathBuf::from(dir).join("recovery.jsonl"))
            {
                let _ = writeln!(f, "{line}");
            }
        }
    }

    pub fn logs(&self) -> Vec<(u16, String)> {
        self.instances
            .iter()
            .map(|inst| {
                (
                    inst.port,
                    std::fs::read_to_string(inst.dir.join("server.log")).unwrap_or_default(),
                )
            })
            .collect()
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        let artifacts = std::env::var("CHECKGATE_TEST_ARTIFACTS").ok();
        if std::thread::panicking() || artifacts.is_some() {
            for (port, log) in self.logs() {
                if std::thread::panicking() {
                    let tail: Vec<&str> = log.lines().rev().take(60).collect();
                    eprintln!("---- {} instance :{port} log tail ----", self.name);
                    for line in tail.iter().rev() {
                        eprintln!("{line}");
                    }
                }
                if let Some(dir) = &artifacts {
                    let _ = std::fs::create_dir_all(dir);
                    let _ = std::fs::write(
                        PathBuf::from(dir).join(format!("{}-{port}.log", self.name)),
                        &log,
                    );
                }
            }
        }
        for inst in &mut self.instances {
            inst.server.take();
        }
    }
}

pub fn flag_body(key: &str, enabled: bool) -> Value {
    json!({ "key": key, "is_enabled": enabled, "flag_type": "boolean", "rules": [] })
}

/// How a client's flag set differs from the committed state, if it does.
/// A client that learned a flag over the stream must hold its exact
/// revision; one that polled a snapshot (no revisions) must match its value.
pub fn divergence(sdk: &SimSdk, truth: &HashMap<String, (i64, bool)>) -> Option<String> {
    let (flags, revisions) = sdk.snapshot();
    if !sdk.with(|s| s.ready) {
        return Some("not ready".into());
    }
    let mut diffs = Vec::new();
    for (key, (revision, enabled)) in truth {
        match flags.get(key) {
            None => diffs.push(format!("{key}: missing (db rev {revision})")),
            Some(flag) => {
                let held = flag["is_enabled"].as_bool();
                if held != Some(*enabled) {
                    diffs.push(format!("{key}: is_enabled {held:?}, db {enabled}"));
                }
                if let Some(k) = revisions.get(key)
                    && (k.revision != *revision || k.deleted)
                {
                    diffs.push(format!(
                        "{key}: rev {} (deleted {}), db {revision}",
                        k.revision, k.deleted
                    ));
                }
            }
        }
    }
    for key in flags.keys() {
        if !truth.contains_key(key) {
            diffs.push(format!("{key}: present but deleted in db"));
        }
    }
    (!diffs.is_empty()).then(|| diffs.join("; "))
}
