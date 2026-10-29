#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! First-run setup: exactly one authorized request may claim an installation,
//! however many arrive at once and on however many instances.

mod common;

use common::*;
use serde_json::{Value, json};

async fn post_concurrently(servers: &[&Server], attempts_per_server: usize) -> Vec<(String, u16)> {
    let mut tasks = Vec::new();
    for (s, server) in servers.iter().enumerate() {
        for i in 0..attempts_per_server {
            let base = server.base.clone();
            let email = format!("racer-{s}-{i}@acme.test");
            tasks.push(tokio::spawn(async move {
                let r = post_setup(
                    &session_client(),
                    &base,
                    Some(SETUP_TOKEN),
                    &setup_body(&email, &format!("password-{s}-{i}")),
                )
                .await;
                (email, r.status().as_u16())
            }));
        }
    }
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap());
    }
    results
}

/// Asserts exactly one winner and that the database holds only its identity.
async fn assert_single_winner(db_url: &str, server: &Server, results: &[(String, u16)]) {
    let winners: Vec<&(String, u16)> = results.iter().filter(|(_, s)| *s == 200).collect();
    assert_eq!(winners.len(), 1, "exactly one setup wins: {results:?}");
    for (email, status) in results {
        assert!(
            *status == 200 || *status == 404,
            "{email}: losers see setup already complete, got {status}"
        );
    }
    let winner = &winners[0].0;

    let pool = sqlx::PgPool::connect(db_url).await.unwrap();
    let users: Vec<(String, String)> = sqlx::query_as("SELECT email, role FROM users")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(users, vec![(winner.clone(), "admin".to_string())]);
    pool.close().await;

    // The winner's credentials work; a loser's were never stored.
    let (s, i) = winner
        .trim_start_matches("racer-")
        .trim_end_matches("@acme.test")
        .split_once('-')
        .unwrap();
    login(server, winner, &format!("password-{s}-{i}")).await;
    if let Some((loser, _)) = results.iter().find(|(_, s)| *s == 404) {
        request(
            &session_client(),
            "POST",
            &server.url("/api/auth/login"),
            Some(json!({ "email": loser, "password": "password-0-0" })),
            401,
        )
        .await;
    }
}

#[tokio::test]
async fn two_concurrent_setups_have_one_winner() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_setup_two").await;
    let server = Server::spawn(&db, &env.redis_url, &[]).await;
    let results = post_concurrently(&[&server], 2).await;
    assert_single_winner(&db, &server, &results).await;
}

#[tokio::test]
async fn many_concurrent_setups_have_one_winner() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_setup_many").await;
    let server = Server::spawn(&db, &env.redis_url, &[]).await;
    let results = post_concurrently(&[&server], 12).await;
    assert_single_winner(&db, &server, &results).await;
}

#[tokio::test]
async fn concurrent_setups_across_instances_have_one_winner() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_setup_multi").await;
    let a = Server::spawn(&db, &env.redis_url, &[]).await;
    let b = Server::spawn(&db, &env.redis_url, &[]).await;
    let results = post_concurrently(&[&a, &b], 6).await;
    assert_single_winner(&db, &b, &results).await;
}

#[tokio::test]
async fn concurrent_setups_have_one_winner_under_repeatable_read_default() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_setup_rr").await;
    // Setup must not rely on the database's default isolation level: under
    // REPEATABLE READ a snapshot taken before the lock wait would miss the
    // winner's commit.
    let pool = sqlx::PgPool::connect(&db).await.unwrap();
    sqlx::query("ALTER DATABASE cg_setup_rr SET default_transaction_isolation = 'repeatable read'")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let server = Server::spawn(&db, &env.redis_url, &[]).await;
    let results = post_concurrently(&[&server], 8).await;
    assert_single_winner(&db, &server, &results).await;
}

#[tokio::test]
async fn setup_requires_a_valid_token_and_is_one_time() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_setup_token").await;
    let server = Server::spawn(&db, &env.redis_url, &[]).await;
    let anon = reqwest::Client::new();
    let body = setup_body(ADMIN_EMAIL, ADMIN_PASSWORD);

    // Missing, empty and wrong tokens are refused, for the key and the claim.
    for token in [None, Some(""), Some("wrong-token-0123456789abcdef")] {
        let r = post_setup(&anon, &server.base, token, &body).await;
        assert_eq!(r.status(), 401, "setup with token {token:?}");
        let mut req = anon.get(server.url("/api/setup/key"));
        if let Some(token) = token {
            req = req.header("X-Checkgate-Setup-Token", token);
        }
        assert_eq!(
            req.send().await.unwrap().status(),
            401,
            "key with {token:?}"
        );
    }
    // The token is a header, never a query parameter.
    let r = anon
        .get(server.url(&format!("/api/setup/key?setup_token={SETUP_TOKEN}")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "query-string token is ignored");
    // A refused attempt changed nothing.
    let workspace: Value = anon
        .get(server.url("/api/auth/workspace"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(workspace["is_setup_complete"], false);

    let (admin, _, _) = complete_setup(&server).await;
    request(&admin, "GET", &server.url("/api/auth/me"), None, 200).await;
    login(&server, ADMIN_EMAIL, ADMIN_PASSWORD).await;

    // Repeated setup — even with the valid token and the same email — is a 404
    // and does not touch the established account.
    let takeover = setup_body(ADMIN_EMAIL, "attacker-password");
    let r = post_setup(
        &session_client(),
        &server.base,
        Some(SETUP_TOKEN),
        &takeover,
    )
    .await;
    assert_eq!(r.status(), 404, "setup after completion");
    let r = anon
        .get(server.url("/api/setup/key"))
        .header("X-Checkgate-Setup-Token", SETUP_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404, "setup key after completion");
    login(&server, ADMIN_EMAIL, ADMIN_PASSWORD).await;
    request(
        &session_client(),
        "POST",
        &server.url("/api/auth/login"),
        Some(json!({ "email": ADMIN_EMAIL, "password": "attacker-password" })),
        401,
    )
    .await;
}

#[tokio::test]
async fn generated_tokens_work_on_every_instance_and_never_reach_the_log() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_setup_generated").await;
    let generated = [("SETUP_TOKEN", None)];
    let a = Server::spawn(&db, &env.redis_url, &generated).await;
    let b = Server::spawn(&db, &env.redis_url, &generated).await;

    let read_token = |server: &Server| {
        let path = server.dir.join("checkgate-setup-token");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "token file is owner-only");
        }
        std::fs::read_to_string(path).unwrap().trim().to_string()
    };
    let token_a = read_token(&a);
    let token_b = read_token(&b);
    assert_eq!(token_a.len(), 64);
    assert_ne!(token_a, token_b, "each instance generates its own token");

    // Instance B accepts instance A's token: hashes live in the shared database.
    let r = reqwest::Client::new()
        .get(b.url("/api/setup/key"))
        .header("X-Checkgate-Setup-Token", &token_a)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "cross-instance token");

    let r = post_setup(
        &session_client(),
        &a.base,
        Some(&token_b),
        &setup_body(ADMIN_EMAIL, ADMIN_PASSWORD),
    )
    .await;
    assert_eq!(r.status(), 200, "setup with generated token");

    let pool = sqlx::PgPool::connect(&db).await.unwrap();
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM settings WHERE starts_with(key, 'setup_token:')")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 0, "setup revokes every generated token");
    pool.close().await;
    assert!(
        !a.dir.join("checkgate-setup-token").exists(),
        "the instance that ran setup removes its token file"
    );

    // B's file is stale now; B removes it on its next start.
    assert!(b.dir.join("checkgate-setup-token").exists());
    let b = b.restart(&db, &env.redis_url, &generated).await;
    assert!(!b.dir.join("checkgate-setup-token").exists());

    for (server, token) in [(&a, &token_a), (&b, &token_b)] {
        let log = server.log();
        assert!(log.contains("setup token"), "log mentions the token file");
        assert!(!log.contains(token.as_str()), "token value is never logged");
    }
}

#[tokio::test]
async fn failed_setup_rolls_back_completely() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_setup_rollback").await;
    let generated = [("SETUP_TOKEN", None)];
    let server = Server::spawn(&db, &env.redis_url, &generated).await;
    let token = std::fs::read_to_string(server.dir.join("checkgate-setup-token"))
        .unwrap()
        .trim()
        .to_string();

    // Fail the last write setup makes before marking itself complete.
    let pool = sqlx::PgPool::connect(&db).await.unwrap();
    sqlx::query(
        "CREATE FUNCTION fail_setup() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN RAISE EXCEPTION 'injected setup failure'; END $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER fail_setup BEFORE UPDATE ON projects FOR EACH ROW EXECUTE FUNCTION fail_setup()")
        .execute(&pool)
        .await
        .unwrap();

    let body = setup_body(ADMIN_EMAIL, ADMIN_PASSWORD);
    let r = post_setup(&session_client(), &server.base, Some(&token), &body).await;
    assert_eq!(r.status(), 500, "injected failure surfaces");

    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(users, 0, "admin insert rolled back");
    let complete: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'setup_complete'")
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert_ne!(
        complete.as_deref(),
        Some("true"),
        "setup not marked complete"
    );
    let workspace: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'workspace_name'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(workspace, "", "workspace name rolled back");
    let tokens: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM settings WHERE starts_with(key, 'setup_token:')")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(tokens, 1, "token revocation rolled back too");

    // Once the fault is gone, the same token completes setup.
    sqlx::query("DROP TRIGGER fail_setup ON projects")
        .execute(&pool)
        .await
        .unwrap();
    let r = post_setup(&session_client(), &server.base, Some(&token), &body).await;
    assert_eq!(r.status(), 200, "retry after rollback");
    login(&server, ADMIN_EMAIL, ADMIN_PASSWORD).await;
    pool.close().await;
}

#[tokio::test]
async fn setup_never_overwrites_an_account_with_a_password() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_setup_existing").await;
    let server = Server::spawn(&db, &env.redis_url, &[]).await;
    let pool = sqlx::PgPool::connect(&db).await.unwrap();
    // A pre-password-auth row may be claimed; an account with a password not.
    sqlx::query(
        "INSERT INTO users (name, email, role, password_hash) VALUES \
         ('Legacy', 'legacy@acme.test', 'viewer', NULL), \
         ('Owner', 'owner@acme.test', 'viewer', 'not-a-real-hash')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let r = post_setup(
        &session_client(),
        &server.base,
        Some(SETUP_TOKEN),
        &setup_body("owner@acme.test", ADMIN_PASSWORD),
    )
    .await;
    assert_eq!(r.status(), 409);
    let (role, hash): (String, String) =
        sqlx::query_as("SELECT role, password_hash FROM users WHERE email = 'owner@acme.test'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (role.as_str(), hash.as_str()),
        ("viewer", "not-a-real-hash")
    );

    let r = post_setup(
        &session_client(),
        &server.base,
        Some(SETUP_TOKEN),
        &setup_body("legacy@acme.test", ADMIN_PASSWORD),
    )
    .await;
    assert_eq!(r.status(), 200, "passwordless legacy row is claimable");
    login(&server, "legacy@acme.test", ADMIN_PASSWORD).await;
    pool.close().await;
}

#[tokio::test]
async fn short_setup_token_is_refused_at_startup() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_setup_short").await;
    let log = expect_startup_failure(&db, &env.redis_url, &[("SETUP_TOKEN", Some("short"))]).await;
    assert!(
        log.contains("SETUP_TOKEN"),
        "error names the setting: {log}"
    );
}
