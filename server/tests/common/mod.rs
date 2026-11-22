//! Shared harness for server integration tests that spawn the real binary.
//!
//! Unlike `integration.rs` and `e2e_sse.rs`, which reset the `public` schema of
//! the database they are given, these tests each create a database of their
//! own next to it, so tests in one binary can run in parallel.
//!
//! Requires `CHECKGATE_TEST_DATABASE_URL` and `CHECKGATE_TEST_REDIS_URL`; the
//! database role must be allowed to `CREATE DATABASE`. Without them the tests
//! skip, unless `CHECKGATE_REQUIRE_INTEGRATION` is set.
#![allow(
    dead_code,
    reason = "each test binary compiles this module and uses a different subset of it"
)]

pub mod chaos;
pub mod cluster;
pub mod sdk;

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

pub const SETUP_TOKEN: &str = "test-setup-token-0123456789abcdef";
pub const ADMIN_EMAIL: &str = "admin@acme.test";
pub const ADMIN_PASSWORD: &str = "supersecret1";
const SESSION_SECRET: &str = "test-secret-0123456789-0123456789-0123456789-0123456789";

pub struct TestEnv {
    pub admin_db_url: String,
    pub redis_url: String,
}

pub fn test_env() -> Option<TestEnv> {
    match (
        std::env::var("CHECKGATE_TEST_DATABASE_URL"),
        std::env::var("CHECKGATE_TEST_REDIS_URL"),
    ) {
        (Ok(admin_db_url), Ok(redis_url)) => Some(TestEnv {
            admin_db_url,
            redis_url,
        }),
        _ => {
            assert!(
                std::env::var("CHECKGATE_REQUIRE_INTEGRATION").is_err(),
                "CHECKGATE_REQUIRE_INTEGRATION is set but CHECKGATE_TEST_DATABASE_URL / \
                 CHECKGATE_TEST_REDIS_URL are not"
            );
            eprintln!(
                "SKIP: set CHECKGATE_TEST_DATABASE_URL and CHECKGATE_TEST_REDIS_URL to run server integration tests"
            );
            None
        }
    }
}

impl TestEnv {
    /// Creates (or recreates) an empty database named `name` and returns its URL.
    pub async fn fresh_database(&self, name: &str) -> String {
        let pool = sqlx::PgPool::connect(&self.admin_db_url)
            .await
            .expect("connect to test database");
        sqlx::query(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
            .execute(&pool)
            .await
            .expect("drop test database");
        sqlx::query(&format!("CREATE DATABASE \"{name}\""))
            .execute(&pool)
            .await
            .expect("create test database (the role needs CREATEDB)");
        pool.close().await;
        let mut url = reqwest::Url::parse(&self.admin_db_url).expect("database URL");
        url.set_path(&format!("/{name}"));
        url.to_string()
    }
}

/// Kills the spawned server when dropped (including on panic).
pub struct Server {
    pub base: String,
    /// Working directory; a generated setup token file lands here.
    pub dir: PathBuf,
    child: Child,
    port: u16,
    /// Where `/metrics` listens, when `METRICS_ADDR` was set.
    metrics: Option<std::net::SocketAddr>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A port that was free a moment ago. Only for addresses a test hands to
/// something it does not start itself (an unreachable webhook target, say):
/// nothing reserves the port after this returns, so a server must never be
/// started on it — two parallel tests can get the same port, and a server
/// that loses the bind used to be mistaken for the one that won it
/// (`reliability::g2`, v0.2.0). Servers bind port 0 instead; see
/// [`Server::spawn`].
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A working directory no other test (or earlier run) uses.
pub fn unique_dir(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "checkgate-test-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// How a spawned process's startup ended, read from its own log lines.
enum Startup {
    /// It is listening: the server port and, if configured, the metrics address.
    Listening(u16, Option<std::net::SocketAddr>),
    /// It could not bind (address in use); it exits on its own.
    AddrInUse(String),
    /// It exited (or never got as far as listening) for another reason.
    Failed(String),
}

/// Bounded attempts at restarting on a fixed port that another process took
/// in the meantime. Each attempt waits for the losing process to exit.
const RESTART_BIND_ATTEMPTS: u32 = 5;

impl Server {
    /// Spawns the server binary on a port the operating system assigns and
    /// waits until `/health` answers.
    /// `extra_env` overrides the defaults; a value of `None` removes a variable.
    ///
    /// The server binds port 0 itself and reports the address in its log, so
    /// no other process can take the port between choosing and binding it.
    pub async fn spawn(db_url: &str, redis_url: &str, extra_env: &[(&str, Option<&str>)]) -> Self {
        Self::spawn_in(&unique_dir("server"), 0, db_url, redis_url, extra_env).await
    }

    /// Like [`Server::spawn`], in `dir` (to restart an instance) and on
    /// `port` (`0`: the operating system picks one), appending its stdout —
    /// the JSON log — to `dir/server.log`. A fixed port that is taken is
    /// retried a few times, then the test fails with the process's log.
    pub async fn spawn_in(
        dir: &Path,
        port: u16,
        db_url: &str,
        redis_url: &str,
        extra_env: &[(&str, Option<&str>)],
    ) -> Self {
        let mut last = String::new();
        for attempt in 1..=RESTART_BIND_ATTEMPTS {
            match Self::start(dir, port, db_url, redis_url, extra_env).await {
                Ok(server) => {
                    server.wait_healthy().await;
                    return server;
                }
                Err(Startup::AddrInUse(log)) if port != 0 => {
                    eprintln!(
                        "port {port} is held by another process (attempt {attempt}/{RESTART_BIND_ATTEMPTS}); retrying"
                    );
                    last = log;
                    tokio::time::sleep(Duration::from_millis(200 * u64::from(attempt))).await;
                }
                Err(Startup::AddrInUse(log) | Startup::Failed(log)) => {
                    panic!("server in {} did not start:\n{log}", dir.display())
                }
                Err(Startup::Listening(..)) => unreachable!(),
            }
        }
        panic!(
            "server could not bind port {port} after {RESTART_BIND_ATTEMPTS} attempts \
             (another process holds it):\n{last}"
        );
    }

    /// Starts the process and waits until *it* is listening — not merely
    /// until something answers on the port. Returns the process's log on
    /// failure.
    async fn start(
        dir: &Path,
        port: u16,
        db_url: &str,
        redis_url: &str,
        extra_env: &[(&str, Option<&str>)],
    ) -> Result<Self, Startup> {
        let log_path = dir.join("server.log");
        let offset = std::fs::metadata(&log_path).map_or(0, |m| m.len());
        let mut child = command(dir, port, db_url, redis_url, extra_env)
            .spawn()
            .expect("spawn server binary");
        let wants_metrics = extra_env
            .iter()
            .any(|(k, v)| *k == "METRICS_ADDR" && v.is_some_and(|v| !v.is_empty()));
        match wait_listening(&mut child, &log_path, offset, wants_metrics).await {
            Startup::Listening(bound, metrics) => {
                assert!(
                    port == 0 || bound == port,
                    "server bound {bound}, asked for {port}"
                );
                Ok(Server {
                    base: format!("http://127.0.0.1:{bound}"),
                    dir: dir.to_path_buf(),
                    child,
                    port: bound,
                    metrics,
                })
            }
            other => {
                let _ = child.kill();
                let _ = child.wait();
                Err(other)
            }
        }
    }

    /// Restarts the process in place: same port, directory and environment.
    pub async fn restart(
        self,
        db_url: &str,
        redis_url: &str,
        extra_env: &[(&str, Option<&str>)],
    ) -> Self {
        let port = self.port;
        let dir = self.dir.clone();
        drop(self);
        Self::spawn_in(&dir, port, db_url, redis_url, extra_env).await
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The address `/metrics` listens on. Start the server with
    /// `METRICS_ADDR=127.0.0.1:0` to have the operating system pick the port.
    pub fn metrics_addr(&self) -> String {
        self.metrics
            .expect("server was started without METRICS_ADDR")
            .to_string()
    }

    /// Sends SIGTERM (graceful shutdown) without waiting.
    pub fn terminate(&self) {
        let status = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .expect("run kill");
        assert!(status.success(), "kill -TERM failed");
    }

    /// Waits up to `within` for the process to exit and returns its status.
    pub async fn wait_exit(&mut self, within: Duration) -> Option<std::process::ExitStatus> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return Some(status);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("server.log")).unwrap_or_default()
    }

    /// Like [`Server::spawn_in`], but returns `None` if the process exits
    /// before it is listening, so the caller can decide what to do.
    pub async fn try_spawn_in(
        dir: &Path,
        port: u16,
        db_url: &str,
        redis_url: &str,
        extra_env: &[(&str, Option<&str>)],
    ) -> Option<Self> {
        match Self::start(dir, port, db_url, redis_url, extra_env).await {
            Ok(server) => {
                server.wait_healthy().await;
                Some(server)
            }
            Err(Startup::AddrInUse(log) | Startup::Failed(log)) => {
                eprintln!("server in {} did not start:\n{log}", dir.display());
                None
            }
            Err(Startup::Listening(..)) => unreachable!(),
        }
    }

    pub async fn wait_healthy(&self) {
        let client = reqwest::Client::new();
        for _ in 0..120 {
            if let Ok(resp) = client.get(format!("{}/health", self.base)).send().await
                && resp.status().is_success()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        panic!("server did not become healthy at {}", self.base);
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }
}

/// Reads the log `child` appends to (from `offset`, i.e. this process's lines
/// only) until it reports `Server listening` (and, with `wants_metrics`, the
/// metrics listener, which binds after it), fails to bind, or exits.
async fn wait_listening(
    child: &mut Child,
    log_path: &Path,
    offset: u64,
    wants_metrics: bool,
) -> Startup {
    use std::io::{Read, Seek, SeekFrom};
    let mut seen = String::new();
    // Startup runs migrations; 60 s matches the old health-check budget.
    for _ in 0..1_200 {
        if let Ok(mut f) = std::fs::File::open(log_path)
            && f.seek(SeekFrom::Start(offset)).is_ok()
        {
            seen.clear();
            let _ = f.read_to_string(&mut seen);
        }
        let (mut port, mut metrics) = (None, None);
        for line in seen.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let fields = &v["fields"];
            let addr = || -> std::net::SocketAddr {
                fields["addr"]
                    .as_str()
                    .and_then(|a| a.parse().ok())
                    .expect("listening line carries addr")
            };
            match fields["message"].as_str() {
                Some("Metrics listening on /metrics") => metrics = Some(addr()),
                Some("Server listening") => port = Some(addr().port()),
                Some(m) if m.starts_with("Failed to bind") => {
                    let in_use = fields["error"]
                        .as_str()
                        .is_some_and(|e| e.contains("in use"));
                    // The process exits on its own; wait for it so the next
                    // attempt does not race it.
                    let _ = child.wait();
                    return if in_use {
                        Startup::AddrInUse(seen.clone())
                    } else {
                        Startup::Failed(seen.clone())
                    };
                }
                _ => {}
            }
        }
        if let Some(port) = port
            && (metrics.is_some() || !wants_metrics)
        {
            return Startup::Listening(port, metrics);
        }
        if let Some(status) = child.try_wait().unwrap() {
            return Startup::Failed(format!("exited with {status} before listening:\n{seen}"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Startup::Failed(format!("not listening after 60 s:\n{seen}"))
}

fn command(
    dir: &Path,
    port: u16,
    db_url: &str,
    redis_url: &str,
    extra_env: &[(&str, Option<&str>)],
) -> Command {
    std::fs::create_dir_all(dir).unwrap();
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("server.log"))
        .unwrap();
    // CHECKGATE_TEST_SERVER_BIN runs the suites against another build, e.g.
    // a baseline binary when reproducing a failure an older commit had.
    let bin = std::env::var("CHECKGATE_TEST_SERVER_BIN")
        .unwrap_or_else(|_| env!("CARGO_BIN_EXE_server").to_string());
    let mut cmd = Command::new(bin);
    cmd.env("DATABASE_URL", db_url)
        .env("REDIS_URL", redis_url)
        .env("PORT", port.to_string())
        .env("SESSION_SECRET", SESSION_SECRET)
        .env("COOKIE_SECURE", "false")
        .env("SETUP_TOKEN", SETUP_TOKEN)
        .env("DB_MAX_CONNECTIONS", "5")
        .env("PUBLIC_DIR", dir)
        .env("RUST_LOG", "info")
        .env_remove("TRUSTED_PROXIES")
        .env_remove("SETUP_TOKEN_FILE")
        .current_dir(dir)
        .stdout(Stdio::from(log))
        .stderr(Stdio::inherit());
    for (key, value) in extra_env {
        match value {
            // The harness reads the `server` target's startup lines (see
            // `wait_listening`), whatever level a test chooses.
            Some(value) if *key == "RUST_LOG" => cmd.env(key, format!("{value},server=info")),
            Some(value) => cmd.env(key, value),
            None => cmd.env_remove(key),
        };
    }
    cmd
}

/// Runs the server with a configuration it must refuse, and returns its log.
pub async fn expect_startup_failure(
    db_url: &str,
    redis_url: &str,
    extra_env: &[(&str, Option<&str>)],
) -> String {
    let dir = unique_dir("startup-failure");
    let mut child = command(&dir, 0, db_url, redis_url, extra_env)
        .spawn()
        .expect("spawn server binary");
    for _ in 0..240 {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(!status.success(), "server exited successfully");
            return std::fs::read_to_string(dir.join("server.log")).unwrap_or_default();
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let _ = child.kill();
    panic!("server kept running with an invalid configuration");
}

pub fn session_client() -> reqwest::Client {
    reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap()
}

pub fn setup_body(email: &str, password: &str) -> Value {
    json!({
        "workspace_name": "Acme",
        "project_name": "Web",
        "name": "Admin",
        "email": email,
        "password": password,
    })
}

/// Sends `POST /api/setup/complete` with the given token (if any).
pub async fn post_setup(
    client: &reqwest::Client,
    base: &str,
    token: Option<&str>,
    body: &Value,
) -> reqwest::Response {
    let mut req = client
        .post(format!("{base}/api/setup/complete"))
        .header("X-Checkgate-Request", "true")
        .json(body);
    if let Some(token) = token {
        req = req.header("X-Checkgate-Setup-Token", token);
    }
    req.send().await.unwrap()
}

/// Completes first-run setup. Returns a logged-in admin client, the seeded
/// SDK key and its (Production) environment id.
pub async fn complete_setup(server: &Server) -> (reqwest::Client, String, String) {
    let key: Value = reqwest::Client::new()
        .get(server.url("/api/setup/key"))
        .header("X-Checkgate-Setup-Token", SETUP_TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let admin = session_client();
    let r = post_setup(
        &admin,
        &server.base,
        Some(SETUP_TOKEN),
        &setup_body(ADMIN_EMAIL, ADMIN_PASSWORD),
    )
    .await;
    assert_eq!(r.status(), 200, "setup/complete");
    (
        admin,
        key["key"].as_str().unwrap().to_string(),
        key["environment_id"].as_str().unwrap().to_string(),
    )
}

/// Sends a JSON request, asserts the status and returns the parsed body.
pub async fn request(
    client: &reqwest::Client,
    method: &str,
    url: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let mut req = client
        .request(method.parse().unwrap(), url)
        .header("X-Checkgate-Request", "true");
    if let Some(body) = body {
        req = req.json(&body);
    }
    let response = req.send().await.unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    assert_eq!(status, expected, "{method} {url}: {text}");
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

pub fn bearer_client(token: &str) -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Authorization", format!("Bearer {token}").parse().unwrap());
    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap()
}

pub async fn login(server: &Server, email: &str, password: &str) -> reqwest::Client {
    let client = session_client();
    request(
        &client,
        "POST",
        &server.url("/api/auth/login"),
        Some(json!({ "email": email, "password": password })),
        200,
    )
    .await;
    client
}
