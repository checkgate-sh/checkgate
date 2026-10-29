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
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

impl Server {
    /// Spawns the server binary and waits until `/health` answers.
    /// `extra_env` overrides the defaults; a value of `None` removes a variable.
    pub async fn spawn(db_url: &str, redis_url: &str, extra_env: &[(&str, Option<&str>)]) -> Self {
        let port = free_port();
        let dir = std::env::temp_dir().join(format!("checkgate-test-{port}"));
        // A directory left by an earlier run on the same port is stale.
        let _ = std::fs::remove_dir_all(&dir);
        Self::spawn_in(&dir, port, db_url, redis_url, extra_env).await
    }

    /// Like [`Server::spawn`], reusing `dir` (to restart an instance) and
    /// appending its stdout — the JSON log — to `dir/server.log`.
    pub async fn spawn_in(
        dir: &Path,
        port: u16,
        db_url: &str,
        redis_url: &str,
        extra_env: &[(&str, Option<&str>)],
    ) -> Self {
        let child = command(dir, port, db_url, redis_url, extra_env)
            .spawn()
            .expect("spawn server binary");
        let server = Server {
            base: format!("http://127.0.0.1:{port}"),
            dir: dir.to_path_buf(),
            child,
        };
        server.wait_healthy().await;
        server
    }

    /// Restarts the process in place: same port, directory and environment.
    pub async fn restart(
        self,
        db_url: &str,
        redis_url: &str,
        extra_env: &[(&str, Option<&str>)],
    ) -> Self {
        let port: u16 = self.base.rsplit(':').next().unwrap().parse().unwrap();
        let dir = self.dir.clone();
        drop(self);
        Self::spawn_in(&dir, port, db_url, redis_url, extra_env).await
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("server.log")).unwrap_or_default()
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
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_server"));
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
    let port = free_port();
    let dir = std::env::temp_dir().join(format!("checkgate-test-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    let mut child = command(&dir, port, db_url, redis_url, extra_env)
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
