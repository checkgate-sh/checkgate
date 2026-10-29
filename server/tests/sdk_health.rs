#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! `GET /api/health/connections` only reveals connections of environments the
//! caller may see, and only workspace admins see full client addresses.

mod common;

use common::*;
use serde_json::json;
use std::collections::BTreeSet;

/// Environment ids of the connections the client can see, plus their IPs.
async fn visible(
    client: &reqwest::Client,
    server: &Server,
) -> (BTreeSet<String>, BTreeSet<String>, usize) {
    let list = request(
        client,
        "GET",
        &server.url("/api/health/connections"),
        None,
        200,
    )
    .await;
    let list = list.as_array().unwrap();
    let envs = list
        .iter()
        .map(|c| c["environment_id"].as_str().unwrap_or("<none>").to_string())
        .collect();
    let ips = list
        .iter()
        .map(|c| c["client_ip"].as_str().unwrap().to_string())
        .collect();
    (envs, ips, list.len())
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn connections_are_scoped_to_project_membership() {
    let Some(env) = test_env() else { return };
    let db = env.fresh_database("cg_sdk_health").await;
    let server = Server::spawn(&db, &env.redis_url, &[]).await;
    let (admin, prod_key, prod_env) = complete_setup(&server).await;

    let pool = sqlx::PgPool::connect(&db).await.unwrap();
    let project_a: String =
        sqlx::query_scalar("SELECT project_id::text FROM environments WHERE id = $1::uuid")
            .bind(&prod_env)
            .fetch_one(&pool)
            .await
            .unwrap();
    pool.close().await;

    let create_env = |project: String, slug: &'static str| {
        let admin = admin.clone();
        let url = server.url(&format!("/api/projects/{project}/environments"));
        async move {
            request(
                &admin,
                "POST",
                &url,
                Some(json!({ "name": slug, "slug": slug })),
                200,
            )
            .await["id"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };
    let create_key = |project: String, env_id: String| {
        let admin = admin.clone();
        let url = server.url(&format!("/api/projects/{project}/keys"));
        async move {
            request(
                &admin,
                "POST",
                &url,
                Some(json!({ "name": format!("key-{env_id}"), "environment_id": env_id })),
                200,
            )
            .await["key"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };

    // Project A: Production (seeded) + Staging. Project B: one environment.
    let staging_env = create_env(project_a.clone(), "qa-health").await;
    let staging_key = create_key(project_a.clone(), staging_env.clone()).await;
    let project_b = request(
        &admin,
        "POST",
        &server.url("/api/projects"),
        Some(json!({ "name": "Other" })),
        200,
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let other_env = create_env(project_b.clone(), "other").await;
    let other_key = create_key(project_b.clone(), other_env.clone()).await;

    // Users: a viewer of A, an editor of B only, and a user in no project.
    let mut ids = Vec::new();
    for (email, role) in [
        ("viewer-a@acme.test", "viewer"),
        ("editor-b@acme.test", "editor"),
        ("outsider@acme.test", "viewer"),
    ] {
        let user = request(
            &admin,
            "POST",
            &server.url("/api/users"),
            Some(
                json!({ "name": email, "email": email, "role": role, "password": ADMIN_PASSWORD }),
            ),
            200,
        )
        .await;
        ids.push(user["id"].as_i64().unwrap());
    }
    for (project, user, role) in [
        (&project_a, ids[0], "viewer"),
        (&project_b, ids[1], "editor"),
    ] {
        request(
            &admin,
            "POST",
            &server.url(&format!("/api/projects/{project}/members")),
            Some(json!({ "user_id": user, "role": role })),
            200,
        )
        .await;
    }
    let viewer_a = login(&server, "viewer-a@acme.test", ADMIN_PASSWORD).await;
    let editor_b = login(&server, "editor-b@acme.test", ADMIN_PASSWORD).await;
    let outsider = login(&server, "outsider@acme.test", ADMIN_PASSWORD).await;
    let viewer_pat = request(
        &viewer_a,
        "POST",
        &server.url("/api/tokens"),
        Some(json!({ "name": "health", "scope": "read_only" })),
        200,
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let viewer_pat = bearer_client(&viewer_pat);

    // Open simultaneous streams: three SDK environments plus an admin session.
    let mut streams = Vec::new();
    for key in [&prod_key, &staging_key, &other_key] {
        let r = bearer_client(key)
            .get(server.url("/stream"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        streams.push(r);
    }
    let session_stream = admin.get(server.url("/stream")).send().await.unwrap();
    assert_eq!(session_stream.status(), 200);

    // Workspace admin: everything, with full addresses.
    let (envs, ips, n) = visible(&admin, &server).await;
    assert_eq!(n, 4);
    assert_eq!(envs, set(&[&prod_env, &staging_env, &other_env, "<none>"]));
    assert_eq!(ips, set(&["127.0.0.1"]));

    // Viewer of A: both of A's environments, masked addresses, nothing of B.
    let (envs, ips, _) = visible(&viewer_a, &server).await;
    assert_eq!(envs, set(&[&prod_env, &staging_env]));
    assert_eq!(ips, set(&["127.0.0.0/24"]));
    // A read-only PAT acts as its owner.
    let (envs, _, _) = visible(&viewer_pat, &server).await;
    assert_eq!(envs, set(&[&prod_env, &staging_env]));

    // Editor of B only: just B.
    let (envs, ips, _) = visible(&editor_b, &server).await;
    assert_eq!(envs, set(&[&other_env]));
    assert_eq!(ips, set(&["127.0.0.0/24"]));

    // No membership: nothing at all.
    let (_, _, n) = visible(&outsider, &server).await;
    assert_eq!(n, 0);

    // SDK keys and anonymous callers cannot list connections.
    for key in [&prod_key, &other_key] {
        let r = bearer_client(key)
            .get(server.url("/api/health/connections"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403, "SDK key");
    }
    let r = reqwest::Client::new()
        .get(server.url("/api/health/connections"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "anonymous");

    // Closing a stream removes it from the listing.
    drop(streams.pop());
    let mut remaining = 0;
    for _ in 0..40 {
        remaining = visible(&admin, &server).await.2;
        if remaining == 3 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(remaining, 3, "closed stream unregistered");
    let (envs, _, _) = visible(&editor_b, &server).await;
    assert!(envs.is_empty());

    // Membership changes apply on the next request.
    request(
        &admin,
        "DELETE",
        &server.url(&format!("/api/projects/{project_a}/members/{}", ids[0])),
        None,
        204,
    )
    .await;
    let (_, _, n) = visible(&viewer_a, &server).await;
    assert_eq!(n, 0, "removed member sees nothing");
}
