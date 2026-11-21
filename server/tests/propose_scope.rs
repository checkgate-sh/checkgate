#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! The `propose` personal-access-token scope, against a real server.
//!
//! A `propose` token (meant for AI agents) can read whatever its owner can
//! read, and otherwise only:
//!
//! - `PATCH` a flag, which always becomes a pending change request (`202`),
//!   whether or not the environment requires approval;
//! - withdraw its owner's own pending change requests.
//!
//! Every other write is `403` — including creating a flag (a new flag is
//! served to SDKs at once), approve and reject, so even raw
//! HTTP with the token cannot apply a change. Project RBAC still applies, the
//! owner cannot approve their own request through a session either, and
//! `read_only` / `read_write` behave exactly as before.

mod common;

use common::*;
use serde_json::{Value, json};

const OWNER: &str = "agent-owner@acme.test";
const REVIEWER: &str = "reviewer@acme.test";
const VIEWER: &str = "viewer@acme.test";
const OTHER_EDITOR: &str = "other-editor@acme.test";

struct Fixture {
    server: Server,
    /// Workspace admin (setup user) session.
    admin: reqwest::Client,
    project: String,
    /// Seeded Production environment — `require_approval` on.
    prod: String,
    /// Second environment of the same project — approval NOT required.
    staging: String,
    /// Environment of another project the agent owner is not a member of.
    foreign: String,
    /// Session of the agent's owner (project editor).
    owner: reqwest::Client,
    /// The owner's `propose` token.
    agent: reqwest::Client,
    agent_token: String,
    /// A different project editor, who reviews.
    reviewer: reqwest::Client,
}

async fn create_user(admin: &reqwest::Client, server: &Server, email: &str) -> i64 {
    request(
        admin,
        "POST",
        &server.url("/api/users"),
        Some(
            json!({ "name": email, "email": email, "role": "viewer", "password": ADMIN_PASSWORD }),
        ),
        200,
    )
    .await["id"]
        .as_i64()
        .unwrap()
}

async fn add_member(
    admin: &reqwest::Client,
    server: &Server,
    project: &str,
    user: i64,
    role: &str,
) {
    request(
        admin,
        "POST",
        &server.url(&format!("/api/projects/{project}/members")),
        Some(json!({ "user_id": user, "role": role })),
        200,
    )
    .await;
}

async fn mint(client: &reqwest::Client, server: &Server, scope: &str) -> String {
    request(
        client,
        "POST",
        &server.url("/api/tokens"),
        Some(json!({ "name": format!("{scope}-token"), "scope": scope })),
        200,
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_string()
}

fn flag(key: &str, enabled: bool) -> Value {
    json!({ "key": key, "is_enabled": enabled, "rollout_percentage": 100, "description": "seed", "rules": [] })
}

async fn fixture(name: &str) -> Option<Fixture> {
    let env = test_env()?;
    let db = env.fresh_database(name).await;
    let server = Server::spawn(&db, &env.redis_url, &[]).await;
    let (admin, _sdk_key, prod) = complete_setup(&server).await;

    let project = request(&admin, "GET", &server.url("/api/projects"), None, 200).await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let staging = request(
        &admin,
        "POST",
        &server.url(&format!("/api/projects/{project}/environments")),
        Some(json!({ "name": "Staging", "slug": "staging-propose" })),
        200,
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let other_project = request(
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
    let foreign = request(
        &admin,
        "POST",
        &server.url(&format!("/api/projects/{other_project}/environments")),
        Some(json!({ "name": "Elsewhere", "slug": "elsewhere" })),
        200,
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();

    for env_id in [&prod, &staging, &foreign] {
        request(
            &admin,
            "POST",
            &server.url(&format!("/api/environments/{env_id}/flags")),
            Some(flag("checkout", false)),
            200,
        )
        .await;
    }
    request(
        &admin,
        "POST",
        &server.url(&format!(
            "/api/projects/{project}/environments/{prod}/require-approval"
        )),
        Some(json!({ "require_approval": true })),
        200,
    )
    .await;

    let owner_id = create_user(&admin, &server, OWNER).await;
    let reviewer_id = create_user(&admin, &server, REVIEWER).await;
    add_member(&admin, &server, &project, owner_id, "editor").await;
    add_member(&admin, &server, &project, reviewer_id, "editor").await;

    let owner = login(&server, OWNER, ADMIN_PASSWORD).await;
    let reviewer = login(&server, REVIEWER, ADMIN_PASSWORD).await;
    let agent_token = mint(&owner, &server, "propose").await;
    let agent = bearer_client(&agent_token);

    Some(Fixture {
        server,
        admin,
        project,
        prod,
        staging,
        foreign,
        owner,
        agent,
        agent_token,
        reviewer,
    })
}

impl Fixture {
    fn url(&self, path: &str) -> String {
        self.server.url(path)
    }

    fn flag_url(&self, env: &str, key: &str) -> String {
        self.url(&format!("/api/environments/{env}/flags/{key}"))
    }

    fn flags_url(&self, env: &str) -> String {
        self.url(&format!("/api/environments/{env}/flags"))
    }

    fn cr_url(&self, env: &str, suffix: &str) -> String {
        self.url(&format!("/api/environments/{env}/change-requests{suffix}"))
    }

    async fn get_flag(&self, env: &str, key: &str) -> Value {
        request(&self.admin, "GET", &self.flag_url(env, key), None, 200).await
    }

    async fn status(
        &self,
        client: &reqwest::Client,
        method: &str,
        url: &str,
        body: Option<Value>,
    ) -> u16 {
        let mut req = client
            .request(method.parse().unwrap(), url)
            .header("X-Checkgate-Request", "true");
        if let Some(body) = body {
            req = req.json(&body);
        }
        req.send().await.unwrap().status().as_u16()
    }
}

#[tokio::test]
async fn propose_patch_always_queues_and_only_someone_else_can_approve() {
    let Some(f) = fixture("cg_propose_patch").await else {
        return;
    };
    let before_staging = f.get_flag(&f.staging, "checkout").await;
    let before_prod = f.get_flag(&f.prod, "checkout").await;

    // Both an unprotected and a protected environment: 202 + pending CR.
    let mut crs = Vec::new();
    for env in [&f.staging, &f.prod] {
        let cr = request(
            &f.agent,
            "PATCH",
            &f.flag_url(env, "checkout"),
            Some(json!({ "is_enabled": true, "rollout_percentage": 25 })),
            202,
        )
        .await;
        assert_eq!(cr["status"], "pending", "{cr}");
        assert_eq!(cr["requested_by"], OWNER, "{cr}");
        assert_eq!(cr["environment_id"].as_str().unwrap(), env.as_str());
        crs.push(cr["id"].as_i64().unwrap());
    }
    // Nothing changed.
    assert_eq!(f.get_flag(&f.staging, "checkout").await, before_staging);
    assert_eq!(f.get_flag(&f.prod, "checkout").await, before_prod);

    // An invalid patch is refused up front and queues nothing.
    request(
        &f.agent,
        "PATCH",
        &f.flag_url(&f.staging, "checkout"),
        Some(json!({ "rollout_percentage": 150 })),
        422,
    )
    .await;
    request(
        &f.agent,
        "PATCH",
        &f.flag_url(&f.staging, "no-such-flag"),
        Some(json!({ "is_enabled": true })),
        404,
    )
    .await;
    let pending = request(
        &f.agent,
        "GET",
        &f.cr_url(&f.staging, "?status=pending"),
        None,
        200,
    )
    .await;
    assert_eq!(pending.as_array().unwrap().len(), 1, "{pending}");

    // The token cannot approve or reject — anyone's, its own included.
    for id in &crs {
        for env in [&f.staging, &f.prod] {
            for action in ["approve", "reject"] {
                let s = f
                    .status(
                        &f.agent,
                        "POST",
                        &f.cr_url(env, &format!("/{id}/{action}")),
                        Some(json!({})),
                    )
                    .await;
                assert_eq!(s, 403, "{action} {id} in {env}");
            }
        }
    }
    // Nor can its owner, through a session: no self-approval.
    for (env, id) in [(&f.staging, crs[0]), (&f.prod, crs[1])] {
        let s = f
            .status(
                &f.owner,
                "POST",
                &f.cr_url(env, &format!("/{id}/approve")),
                None,
            )
            .await;
        assert_eq!(s, 403, "self-approval in {env}");
    }
    assert_eq!(f.get_flag(&f.staging, "checkout").await, before_staging);

    // A different editor approves → applied.
    let applied = request(
        &f.reviewer,
        "POST",
        &f.cr_url(&f.staging, &format!("/{}/approve", crs[0])),
        None,
        200,
    )
    .await;
    assert_eq!(applied["is_enabled"], true);
    let after = f.get_flag(&f.staging, "checkout").await;
    assert_eq!(after["is_enabled"], true);
    assert_eq!(after["rollout_percentage"], 25);

    // The agent reads back the outcome.
    let all = request(&f.agent, "GET", &f.cr_url(&f.staging, ""), None, 200).await;
    let mine = all
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"].as_i64() == Some(crs[0]))
        .unwrap();
    assert_eq!(mine["status"], "approved");
    assert_eq!(mine["reviewed_by"], REVIEWER);

    // The audit trail names the reviewer as the actor of the applied change.
    let audit = request(
        &f.agent,
        "GET",
        &f.url(&format!("/api/environments/{}/audit", f.staging)),
        None,
        200,
    )
    .await;
    let update = audit
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["action"] == "UPDATE")
        .unwrap_or_else(|| panic!("no UPDATE audit event: {audit}"));
    assert_eq!(update["actor_email"], REVIEWER, "{update}");

    // Withdrawing its own pending request is allowed — once.
    request(
        &f.agent,
        "DELETE",
        &f.cr_url(&f.prod, &format!("/{}", crs[1])),
        None,
        204,
    )
    .await;
    let s = f
        .status(
            &f.agent,
            "DELETE",
            &f.cr_url(&f.prod, &format!("/{}", crs[1])),
            None,
        )
        .await;
    assert_eq!(s, 404);
    assert_eq!(f.get_flag(&f.prod, "checkout").await, before_prod);
}

#[tokio::test]
async fn propose_cancel_is_own_requests_only_even_for_an_admin_owner() {
    let Some(f) = fixture("cg_propose_cancel").await else {
        return;
    };
    // The reviewer opens a CR in the protected environment.
    let theirs = request(
        &f.reviewer,
        "PATCH",
        &f.flag_url(&f.prod, "checkout"),
        Some(json!({ "is_enabled": true })),
        202,
    )
    .await["id"]
        .as_i64()
        .unwrap();
    let url = f.cr_url(&f.prod, &format!("/{theirs}"));

    // The project-editor owner's propose token: not theirs → 404.
    assert_eq!(f.status(&f.agent, "DELETE", &url, None).await, 404);
    // A workspace admin's propose token gets no admin override either.
    let admin_agent = bearer_client(&mint(&f.admin, &f.server, "propose").await);
    assert_eq!(f.status(&admin_agent, "DELETE", &url, None).await, 404);
    // ...and cannot approve, although the admin's session could.
    let approve = f.cr_url(&f.prod, &format!("/{theirs}/approve"));
    assert_eq!(f.status(&admin_agent, "POST", &approve, None).await, 403);
    let pending = request(
        &f.admin,
        "GET",
        &f.cr_url(&f.prod, "?status=pending"),
        None,
        200,
    )
    .await;
    assert_eq!(pending.as_array().unwrap().len(), 1);
    // The admin's session keeps the override.
    request(&f.admin, "DELETE", &url, None, 204).await;
}

#[tokio::test]
async fn propose_cannot_create_flags() {
    let Some(f) = fixture("cg_propose_create").await else {
        return;
    };
    // A created flag is served to SDKs at once — even a disabled one makes
    // code that already reads the key get `false` instead of its own default
    // (review finding F-01) — so a propose token creates nothing: not a new
    // disabled flag, not an enabled one, not over an existing key, in an
    // environment with or without approval.
    let before = f.get_flag(&f.staging, "checkout").await;
    for env in [&f.staging, &f.prod] {
        for body in [
            json!({ "key": "agent-draft", "is_enabled": false, "rollout_percentage": null,
                    "description": "Drafted by an agent", "rules": [], "tags": ["agent"] }),
            json!({ "key": "agent-live", "is_enabled": true, "rollout_percentage": 100,
                    "description": null, "rules": [] }),
            json!({ "key": "checkout", "is_enabled": false, "rollout_percentage": 0,
                    "description": "overwritten", "rules": [] }),
        ] {
            let s = f
                .status(&f.agent, "POST", &f.flags_url(env), Some(body.clone()))
                .await;
            assert_eq!(s, 403, "{env}: {body}");
        }
        for key in ["agent-draft", "agent-live"] {
            let s = f.status(&f.admin, "GET", &f.flag_url(env, key), None).await;
            assert_eq!(s, 404, "{env}: {key} must not exist");
        }
    }
    assert_eq!(f.get_flag(&f.staging, "checkout").await, before);
    let audit = request(
        &f.admin,
        "GET",
        &f.url(&format!("/api/environments/{}/audit", f.staging)),
        None,
        200,
    )
    .await;
    assert!(
        !audit
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "CREATE" && e["actor_email"] == OWNER),
        "no CREATE by the token owner: {audit}"
    );
    // The owner's own session still creates flags as before.
    request(
        &f.owner,
        "POST",
        &f.flags_url(&f.staging),
        Some(
            json!({ "key": "owner-flag", "is_enabled": false, "rollout_percentage": null,
                     "description": null, "rules": [] }),
        ),
        200,
    )
    .await;

    // Cross-project: the owner is not a member of the foreign project.
    let s = f
        .status(
            &f.agent,
            "POST",
            &f.flags_url(&f.foreign),
            Some(flag("agent-x", false)),
        )
        .await;
    assert_eq!(s, 403);
    let s = f
        .status(
            &f.agent,
            "PATCH",
            &f.flag_url(&f.foreign, "checkout"),
            Some(json!({ "is_enabled": true })),
        )
        .await;
    assert_eq!(s, 403);
    let s = f
        .status(&f.agent, "GET", &f.flags_url(&f.foreign), None)
        .await;
    assert_eq!(s, 403);
    let s = f
        .status(&f.agent, "GET", &f.cr_url(&f.foreign, ""), None)
        .await;
    assert_eq!(s, 403);
}

#[tokio::test]
async fn propose_cannot_reach_any_other_write() {
    let Some(f) = fixture("cg_propose_writes").await else {
        return;
    };
    let (p, e) = (f.project.clone(), f.staging.clone());
    // A CR to aim approve/reject at, and a token of the owner's to revoke.
    let cr = request(
        &f.agent,
        "PATCH",
        &f.flag_url(&e, "checkout"),
        Some(json!({ "is_enabled": true })),
        202,
    )
    .await["id"]
        .as_i64()
        .unwrap();
    let other_token = request(
        &f.owner,
        "POST",
        &f.url("/api/tokens"),
        Some(json!({ "name": "other", "scope": "read_only" })),
        200,
    )
    .await["id"]
        .as_i64()
        .unwrap();

    let writes: Vec<(&str, String, Option<Value>)> = vec![
        (
            "DELETE",
            format!("/api/environments/{e}/flags/checkout"),
            None,
        ),
        (
            "POST",
            format!("/api/environments/{e}/flags/checkout/promote"),
            Some(json!({ "target_env_id": f.prod })),
        ),
        (
            "POST",
            format!("/api/environments/{e}/flags/checkout/archive"),
            None,
        ),
        (
            "POST",
            format!("/api/environments/{e}/flags/checkout/unarchive"),
            None,
        ),
        (
            "POST",
            format!("/api/environments/{e}/change-requests/{cr}/approve"),
            None,
        ),
        (
            "POST",
            format!("/api/environments/{e}/change-requests/{cr}/reject"),
            Some(json!({ "reason": "x" })),
        ),
        (
            "POST",
            format!("/api/environments/{e}/flags/checkout/scheduled-changes"),
            Some(
                json!({ "scheduled_at": "2099-01-01T00:00:00Z", "patch": { "is_enabled": true } }),
            ),
        ),
        (
            "POST",
            format!("/api/environments/{e}/segments"),
            Some(json!({ "key": "beta", "name": "Beta", "rules": [] })),
        ),
        (
            "PATCH",
            format!("/api/environments/{e}/segments/beta"),
            Some(json!({ "name": "B" })),
        ),
        (
            "DELETE",
            format!("/api/environments/{e}/segments/beta"),
            None,
        ),
        (
            "POST",
            format!("/api/environments/{e}/experiments"),
            Some(json!({ "key": "x", "name": "x", "flag_key": "checkout", "goal_event_key": "g" })),
        ),
        (
            "POST",
            format!("/api/environments/{e}/webhooks"),
            Some(json!({ "url": "https://hooks.example/x", "events": [] })),
        ),
        (
            "POST",
            format!("/api/environments/{e}/integrations"),
            Some(json!({ "kind": "slack", "url": "https://hooks.example/x" })),
        ),
        (
            "POST",
            format!("/api/environments/{e}/impressions"),
            Some(json!([])),
        ),
        (
            "POST",
            format!("/api/environments/{e}/events"),
            Some(json!([])),
        ),
        (
            "POST",
            format!("/api/projects/{p}/environments"),
            Some(json!({ "name": "N", "slug": "n" })),
        ),
        (
            "DELETE",
            format!("/api/projects/{p}/environments/{e}"),
            None,
        ),
        (
            "POST",
            format!("/api/projects/{p}/environments/{}/require-approval", f.prod),
            Some(json!({ "require_approval": false })),
        ),
        (
            "POST",
            format!("/api/projects/{p}/environments/{e}/default"),
            None,
        ),
        (
            "POST",
            format!("/api/projects/{p}/keys"),
            Some(json!({ "name": "k", "environment_id": e })),
        ),
        (
            "POST",
            "/api/users".to_string(),
            Some(
                json!({ "name": "n", "email": "n@acme.test", "role": "admin", "password": ADMIN_PASSWORD }),
            ),
        ),
        (
            "POST",
            "/api/projects".to_string(),
            Some(json!({ "name": "P" })),
        ),
        (
            "PATCH",
            format!("/api/projects/{p}"),
            Some(json!({ "name": "Renamed" })),
        ),
        (
            "POST",
            format!("/api/projects/{p}/members"),
            Some(json!({ "user_id": 1, "role": "admin" })),
        ),
        ("DELETE", format!("/api/tokens/{other_token}"), None),
        (
            "POST",
            "/api/tokens".to_string(),
            Some(json!({ "name": "escalate", "scope": "read_write" })),
        ),
    ];
    // Both the project editor's token and a workspace admin's: for the admin
    // every one of these would succeed with a session, so a 403 here is the
    // scope cap and not the role check.
    let admin_agent = bearer_client(&mint(&f.admin, &f.server, "propose").await);
    for (who, client) in [("editor", &f.agent), ("admin", &admin_agent)] {
        for (method, path, body) in &writes {
            let s = f.status(client, method, &f.url(path), body.clone()).await;
            assert_eq!(s, 403, "{who}: {method} {path}");
        }
    }
    // The flag and the CR are untouched; the approval guard is still on.
    let checkout = f.get_flag(&e, "checkout").await;
    assert_eq!(checkout["is_enabled"], false);
    assert!(checkout.get("archived_at").is_none(), "{checkout}");
    let pending = request(&f.admin, "GET", &f.cr_url(&e, "?status=pending"), None, 200).await;
    assert_eq!(pending.as_array().unwrap().len(), 1);
    let envs = request(
        &f.admin,
        "GET",
        &f.url(&format!("/api/projects/{p}/environments")),
        None,
        200,
    )
    .await;
    let prod = envs
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == f.prod.as_str())
        .unwrap();
    assert_eq!(prod["require_approval"], true);

    // Minting: a propose token may create propose or read_only tokens only.
    let child = mint(&f.agent, &f.server, "propose").await;
    mint(&f.agent, &f.server, "read_only").await;
    // ...and a read_only token, read_only only.
    let ro = bearer_client(&mint(&f.owner, &f.server, "read_only").await);
    for scope in ["propose", "read_write"] {
        let s = f
            .status(
                &ro,
                "POST",
                &f.url("/api/tokens"),
                Some(json!({ "name": "x", "scope": scope })),
            )
            .await;
        assert_eq!(s, 403, "read_only minting {scope}");
    }
    let s = f
        .status(
            &f.agent,
            "POST",
            &f.url("/api/tokens"),
            Some(json!({ "name": "x", "scope": "admin" })),
        )
        .await;
    assert_eq!(s, 422, "unknown scope");
    // The child token is itself only a propose token.
    let child = bearer_client(&child);
    let s = f
        .status(
            &child,
            "POST",
            &f.cr_url(&e, &format!("/{cr}/approve")),
            None,
        )
        .await;
    assert_eq!(s, 403);

    // Reads keep working, the token value never appears in a listing.
    for path in [
        format!("/api/environments/{e}/flags"),
        format!("/api/environments/{e}/flags/checkout"),
        format!("/api/environments/{e}/change-requests"),
        format!("/api/environments/{e}/audit"),
        format!("/api/projects/{p}/environments"),
        "/api/projects".to_string(),
        "/api/health/connections".to_string(),
    ] {
        assert_eq!(
            f.status(&f.agent, "GET", &f.url(&path), None).await,
            200,
            "{path}"
        );
    }
    let tokens = request(&f.agent, "GET", &f.url("/api/tokens"), None, 200).await;
    assert!(!tokens.to_string().contains(&f.agent_token));
    let scopes: Vec<&str> = tokens
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["scope"].as_str().unwrap())
        .collect();
    assert!(scopes.contains(&"propose"), "{tokens}");
}

#[tokio::test]
async fn propose_respects_project_rbac() {
    let Some(f) = fixture("cg_propose_rbac").await else {
        return;
    };
    let viewer_id = create_user(&f.admin, &f.server, VIEWER).await;
    add_member(&f.admin, &f.server, &f.project, viewer_id, "viewer").await;
    let viewer = login(&f.server, VIEWER, ADMIN_PASSWORD).await;
    let viewer_agent = bearer_client(&mint(&viewer, &f.server, "propose").await);

    // A viewer's propose token can read but not propose or create.
    assert_eq!(
        f.status(&viewer_agent, "GET", &f.flags_url(&f.staging), None)
            .await,
        200
    );
    let s = f
        .status(
            &viewer_agent,
            "PATCH",
            &f.flag_url(&f.staging, "checkout"),
            Some(json!({ "is_enabled": true })),
        )
        .await;
    assert_eq!(s, 403);
    let s = f
        .status(
            &viewer_agent,
            "POST",
            &f.flags_url(&f.staging),
            Some(flag("viewer-draft", false)),
        )
        .await;
    assert_eq!(s, 403);

    // An editor of another project only: no access to this one at all.
    let other_id = create_user(&f.admin, &f.server, OTHER_EDITOR).await;
    let foreign_project: String = {
        let envs = request(&f.admin, "GET", &f.url("/api/projects"), None, 200).await;
        envs.as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] != f.project.as_str())
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    add_member(&f.admin, &f.server, &foreign_project, other_id, "editor").await;
    let other = login(&f.server, OTHER_EDITOR, ADMIN_PASSWORD).await;
    let other_agent = bearer_client(&mint(&other, &f.server, "propose").await);
    let s = f
        .status(
            &other_agent,
            "PATCH",
            &f.flag_url(&f.staging, "checkout"),
            Some(json!({ "is_enabled": true })),
        )
        .await;
    assert_eq!(s, 403);
    assert_eq!(
        f.status(&other_agent, "GET", &f.flags_url(&f.staging), None)
            .await,
        403
    );
    // ...while in its own project it proposes normally.
    request(
        &other_agent,
        "PATCH",
        &f.flag_url(&f.foreign, "checkout"),
        Some(json!({ "is_enabled": true })),
        202,
    )
    .await;
    let pending = request(
        &f.admin,
        "GET",
        &f.cr_url(&f.staging, "?status=pending"),
        None,
        200,
    )
    .await;
    assert_eq!(pending.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn read_only_and_read_write_are_unchanged() {
    let Some(f) = fixture("cg_propose_regression").await else {
        return;
    };
    let ro = bearer_client(&mint(&f.owner, &f.server, "read_only").await);
    let rw = bearer_client(&mint(&f.owner, &f.server, "read_write").await);

    // read_only: reads only.
    assert_eq!(
        f.status(&ro, "GET", &f.flags_url(&f.staging), None).await,
        200
    );
    let s = f
        .status(
            &ro,
            "PATCH",
            &f.flag_url(&f.staging, "checkout"),
            Some(json!({ "is_enabled": true })),
        )
        .await;
    assert_eq!(s, 403);
    let s = f
        .status(
            &ro,
            "POST",
            &f.flags_url(&f.staging),
            Some(flag("ro-new", false)),
        )
        .await;
    assert_eq!(s, 403);
    let cr = request(
        &f.owner,
        "PATCH",
        &f.flag_url(&f.prod, "checkout"),
        Some(json!({ "is_enabled": true })),
        202,
    )
    .await["id"]
        .as_i64()
        .unwrap();
    let s = f
        .status(&ro, "DELETE", &f.cr_url(&f.prod, &format!("/{cr}")), None)
        .await;
    assert_eq!(s, 403);

    // read_write: a PATCH applies directly where approval is not required...
    let applied = request(
        &rw,
        "PATCH",
        &f.flag_url(&f.staging, "checkout"),
        Some(json!({ "is_enabled": true })),
        200,
    )
    .await;
    assert_eq!(applied["is_enabled"], true);
    // ...and queues where it is.
    request(
        &rw,
        "PATCH",
        &f.flag_url(&f.prod, "checkout"),
        Some(json!({ "rollout_percentage": 10 })),
        202,
    )
    .await;
    // POST still upserts an existing key in an unprotected environment, with
    // any enabled state, and is still refused over one in a protected one.
    let replaced = request(
        &rw,
        "POST",
        &f.flags_url(&f.staging),
        Some(
            json!({ "key": "checkout", "is_enabled": true, "rollout_percentage": 50,
                     "description": "replaced", "rules": [] }),
        ),
        200,
    )
    .await;
    assert_eq!(replaced["rollout_percentage"], 50);
    let s = f
        .status(
            &rw,
            "POST",
            &f.flags_url(&f.prod),
            Some(flag("checkout", true)),
        )
        .await;
    assert_eq!(s, 409);
    request(
        &rw,
        "POST",
        &f.flags_url(&f.staging),
        Some(flag("rw-live", true)),
        200,
    )
    .await;
    // Withdrawing its own CR, deleting a flag, minting any scope.
    request(
        &rw,
        "DELETE",
        &f.cr_url(&f.prod, &format!("/{cr}")),
        None,
        204,
    )
    .await;
    request(&rw, "DELETE", &f.flag_url(&f.staging, "rw-live"), None, 204).await;
    for scope in ["read_only", "propose", "read_write"] {
        mint(&rw, &f.server, scope).await;
    }
    // A session is uncapped too.
    mint(&f.owner, &f.server, "read_write").await;
}
