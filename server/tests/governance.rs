#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! Flag creation in an environment that requires approval, against a real
//! server (v0.2.0 governance hardening).
//!
//! Before, `POST /api/environments/{env}/flags` wrote an enabled flag straight
//! into such an environment and SDKs were served it at once. Now:
//!
//! - an *inert* flag (disabled, serving the built-in `false`) is still created
//!   at once — the ordinary way a flag is born;
//! - any other creation is a pending `create` change request (`202`): nothing
//!   is stored in `flags` or served to SDKs until a *different* project
//!   editor/admin approves it;
//! - rejection, cancellation and failed approvals leave the active
//!   configuration untouched;
//! - environments that do not require approval behave exactly as before.
//!
//! Every test checks the SDK snapshot of the protected environment, which is
//! what SDKs actually serve.

mod common;

use common::*;
use serde_json::{Value, json};

const REQUESTER: &str = "requester@acme.test";
const REVIEWER: &str = "reviewer@acme.test";
const OUTSIDER: &str = "outsider@acme.test";

struct Fixture {
    server: Server,
    /// Workspace admin (setup user) session.
    admin: reqwest::Client,
    project: String,
    /// Seeded Production environment — `require_approval` on.
    prod: String,
    /// SDK key of Production.
    prod_sdk: reqwest::Client,
    /// Same project, approval NOT required.
    staging: String,
    /// Environment of another project, where `OUTSIDER` is an editor.
    foreign: String,
    /// Project editor who proposes.
    requester: reqwest::Client,
    /// A different project editor, who reviews.
    reviewer: reqwest::Client,
    reviewer_id: i64,
    /// Editor of the other project only.
    outsider: reqwest::Client,
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

fn enabled(key: &str) -> Value {
    json!({ "key": key, "is_enabled": true, "rollout_percentage": 100,
            "description": "proposed", "rules": [], "tags": ["governance"] })
}

fn inert(key: &str) -> Value {
    json!({ "key": key, "is_enabled": false, "rollout_percentage": 25,
            "description": "born off", "rules": [] })
}

async fn fixture(name: &str) -> Option<Fixture> {
    let env = test_env()?;
    let db = env.fresh_database(name).await;
    let server = Server::spawn(&db, &env.redis_url, &[]).await;
    let (admin, sdk_key, prod) = complete_setup(&server).await;

    let project = request(&admin, "GET", &server.url("/api/projects"), None, 200).await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let staging = request(
        &admin,
        "POST",
        &server.url(&format!("/api/projects/{project}/environments")),
        Some(json!({ "name": "Staging", "slug": "staging-governance" })),
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

    let requester_id = create_user(&admin, &server, REQUESTER).await;
    let reviewer_id = create_user(&admin, &server, REVIEWER).await;
    let outsider_id = create_user(&admin, &server, OUTSIDER).await;
    add_member(&admin, &server, &project, requester_id, "editor").await;
    add_member(&admin, &server, &project, reviewer_id, "editor").await;
    add_member(&admin, &server, &other_project, outsider_id, "editor").await;

    let requester = login(&server, REQUESTER, ADMIN_PASSWORD).await;
    let reviewer = login(&server, REVIEWER, ADMIN_PASSWORD).await;
    let outsider = login(&server, OUTSIDER, ADMIN_PASSWORD).await;

    Some(Fixture {
        prod_sdk: bearer_client(&sdk_key),
        server,
        admin,
        project,
        prod,
        staging,
        foreign,
        requester,
        reviewer,
        reviewer_id,
        outsider,
    })
}

impl Fixture {
    fn url(&self, path: &str) -> String {
        self.server.url(path)
    }

    fn flags_url(&self, env: &str) -> String {
        self.url(&format!("/api/environments/{env}/flags"))
    }

    fn flag_url(&self, env: &str, key: &str) -> String {
        self.url(&format!("/api/environments/{env}/flags/{key}"))
    }

    fn cr_url(&self, env: &str, suffix: &str) -> String {
        self.url(&format!("/api/environments/{env}/change-requests{suffix}"))
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

    /// What SDKs of Production are served for `key`, if anything.
    async fn served(&self, key: &str) -> Option<Value> {
        let snapshot = request(
            &self.prod_sdk,
            "GET",
            &self.url("/flags/snapshot"),
            None,
            200,
        )
        .await;
        snapshot
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["key"] == key)
            .cloned()
    }

    async fn propose(&self, client: &reqwest::Client, body: Value) -> Value {
        let cr = request(client, "POST", &self.flags_url(&self.prod), Some(body), 202).await;
        assert_eq!(cr["action"], "create", "{cr}");
        assert_eq!(cr["status"], "pending", "{cr}");
        cr
    }

    async fn pending(&self, env: &str) -> Vec<Value> {
        request(
            &self.admin,
            "GET",
            &self.cr_url(env, "?status=pending"),
            None,
            200,
        )
        .await
        .as_array()
        .unwrap()
        .clone()
    }

    async fn audit(&self, env: &str, key: &str) -> Vec<Value> {
        request(
            &self.admin,
            "GET",
            &self.url(&format!("/api/environments/{env}/audit?limit=500")),
            None,
            200,
        )
        .await
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["flag_key"] == key)
        .cloned()
        .collect()
    }

    /// The flag is absent everywhere a reader could find it.
    async fn assert_absent(&self, key: &str) {
        assert_eq!(
            self.status(&self.admin, "GET", &self.flag_url(&self.prod, key), None)
                .await,
            404,
            "{key} must not exist in Production"
        );
        assert!(self.served(key).await.is_none(), "{key} must not be served");
        assert!(
            !self
                .audit(&self.prod, key)
                .await
                .iter()
                .any(|e| e["action"] == "CREATE"),
            "no CREATE audited for {key}"
        );
    }
}

/// 1 — an enabled flag (and every other creation that would serve a value)
/// is queued, not stored or served.
#[tokio::test]
async fn enabled_create_in_protected_environment_is_queued_not_served() {
    let Some(f) = fixture("cg_gov_enabled_create").await else {
        return;
    };
    let cr = f.propose(&f.requester, enabled("go-live")).await;
    assert_eq!(cr["flag_key"], "go-live");
    assert_eq!(cr["requested_by"], REQUESTER);
    assert_eq!(
        cr["patch"]["is_enabled"], true,
        "the full body is kept: {cr}"
    );
    assert_eq!(cr["patch"]["tags"], json!(["governance"]), "{cr}");
    f.assert_absent("go-live").await;

    // A disabled flag that serves a value of its own is not inert either.
    for (key, body) in [
        (
            "off-but-true",
            json!({ "key": "off-but-true", "is_enabled": false, "disabled_value": true,
                    "rollout_percentage": null, "description": null, "rules": [] }),
        ),
        (
            "off-variant",
            json!({ "key": "off-variant", "is_enabled": false, "flag_type": "string",
                    "default_value": "new", "disabled_value": "legacy",
                    "rollout_percentage": null, "description": null, "rules": [] }),
        ),
    ] {
        f.propose(&f.requester, body).await;
        f.assert_absent(key).await;
    }
    assert_eq!(f.pending(&f.prod).await.len(), 3);
}

/// 2 — an inert flag is still created at once; enabling it is a change
/// request. Environments without approval are unchanged.
#[tokio::test]
async fn inert_create_applies_at_once_and_unprotected_environments_are_unchanged() {
    let Some(f) = fixture("cg_gov_inert_create").await else {
        return;
    };
    let created = request(
        &f.requester,
        "POST",
        &f.flags_url(&f.prod),
        Some(inert("born-off")),
        200,
    )
    .await;
    assert_eq!(created["is_enabled"], false);
    let served = f.served("born-off").await.expect("an inert flag is served");
    assert_eq!(served["is_enabled"], false);
    // An explicit `disabled_value: false` is the built-in value: still inert.
    request(
        &f.requester,
        "POST",
        &f.flags_url(&f.prod),
        Some(
            json!({ "key": "explicit-false", "is_enabled": false, "disabled_value": false,
                     "rollout_percentage": null, "description": null, "rules": [] }),
        ),
        200,
    )
    .await;
    assert!(f.pending(&f.prod).await.is_empty());

    // Turning it on goes through approval.
    let cr = request(
        &f.requester,
        "PATCH",
        &f.flag_url(&f.prod, "born-off"),
        Some(json!({ "is_enabled": true })),
        202,
    )
    .await;
    assert_eq!(cr["action"], "update", "{cr}");
    assert_eq!(f.served("born-off").await.unwrap()["is_enabled"], false);

    // Staging requires no approval: an enabled create applies at once (200),
    // and replacing an existing key still works there.
    let created = request(
        &f.requester,
        "POST",
        &f.flags_url(&f.staging),
        Some(enabled("go-live")),
        200,
    )
    .await;
    assert_eq!(created["is_enabled"], true);
    let mut replaced = enabled("go-live");
    replaced["rollout_percentage"] = json!(10);
    let replaced = request(
        &f.requester,
        "POST",
        &f.flags_url(&f.staging),
        Some(replaced),
        200,
    )
    .await;
    assert_eq!(replaced["rollout_percentage"], 10);
    assert!(f.pending(&f.staging).await.is_empty());
    // Nothing leaked into Production.
    f.assert_absent("go-live").await;
}

/// 3, 4 — a proposed creation goes live only when a different editor
/// approves it, through the normal workflow, with an audit trail.
#[tokio::test]
async fn creation_goes_live_when_a_second_editor_approves() {
    let Some(f) = fixture("cg_gov_approve").await else {
        return;
    };
    let cr = f.propose(&f.requester, enabled("go-live")).await;
    let id = cr["id"].as_i64().unwrap();
    f.assert_absent("go-live").await;

    let applied = request(
        &f.reviewer,
        "POST",
        &f.cr_url(&f.prod, &format!("/{id}/approve")),
        None,
        200,
    )
    .await;
    assert_eq!(applied["key"], "go-live");
    assert_eq!(applied["is_enabled"], true);
    assert_eq!(applied["tags"], json!(["governance"]));

    let served = f.served("go-live").await.expect("approved flag is served");
    assert_eq!(served["is_enabled"], true);
    assert_eq!(served["rollout_percentage"], 100);
    let stored = request(&f.admin, "GET", &f.flag_url(&f.prod, "go-live"), None, 200).await;
    assert_eq!(stored["tags"], json!(["governance"]));

    let all = request(&f.admin, "GET", &f.cr_url(&f.prod, ""), None, 200).await;
    let row = all
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap();
    assert_eq!(row["status"], "approved");
    assert_eq!(row["reviewed_by"], REVIEWER);
    assert_eq!(row["requested_by"], REQUESTER);

    let audit = f.audit(&f.prod, "go-live").await;
    let creates: Vec<_> = audit.iter().filter(|e| e["action"] == "CREATE").collect();
    assert_eq!(creates.len(), 1, "{audit:?}");
    assert_eq!(creates[0]["actor_email"], REVIEWER);
    assert_eq!(creates[0]["metadata"]["change_request_id"], id);
    assert_eq!(creates[0]["metadata"]["requested_by"], REQUESTER);

    // Approving again is not possible.
    assert_eq!(
        f.status(
            &f.admin,
            "POST",
            &f.cr_url(&f.prod, &format!("/{id}/approve")),
            None
        )
        .await,
        409
    );
}

/// 5 — nobody approves their own creation, a workspace admin included.
#[tokio::test]
async fn self_approval_of_a_creation_is_refused() {
    let Some(f) = fixture("cg_gov_self_approve").await else {
        return;
    };
    let mine = f.propose(&f.requester, enabled("mine")).await["id"]
        .as_i64()
        .unwrap();
    assert_eq!(
        f.status(
            &f.requester,
            "POST",
            &f.cr_url(&f.prod, &format!("/{mine}/approve")),
            None
        )
        .await,
        403
    );
    // A read_write token of the requester is the requester too.
    let token = bearer_client(&mint(&f.requester, &f.server, "read_write").await);
    assert_eq!(
        f.status(
            &token,
            "POST",
            &f.cr_url(&f.prod, &format!("/{mine}/approve")),
            None
        )
        .await,
        403
    );
    f.assert_absent("mine").await;

    let admins = f.propose(&f.admin, enabled("admins")).await["id"]
        .as_i64()
        .unwrap();
    assert_eq!(
        f.status(
            &f.admin,
            "POST",
            &f.cr_url(&f.prod, &format!("/{admins}/approve")),
            None
        )
        .await,
        403,
        "a workspace admin cannot approve their own creation"
    );
    f.assert_absent("admins").await;
    assert_eq!(f.pending(&f.prod).await.len(), 2);
}

/// 6 — a rejected (or withdrawn) creation changes nothing; the key can be
/// proposed again.
#[tokio::test]
async fn rejected_creation_changes_nothing() {
    let Some(f) = fixture("cg_gov_reject").await else {
        return;
    };
    let id = f.propose(&f.requester, enabled("nope")).await["id"]
        .as_i64()
        .unwrap();
    assert_eq!(
        f.status(
            &f.reviewer,
            "POST",
            &f.cr_url(&f.prod, &format!("/{id}/reject")),
            Some(json!({ "reason": "not yet" }))
        )
        .await,
        204
    );
    f.assert_absent("nope").await;
    assert_eq!(
        f.status(
            &f.admin,
            "POST",
            &f.cr_url(&f.prod, &format!("/{id}/approve")),
            None
        )
        .await,
        409,
        "a rejected creation cannot be approved later"
    );
    f.assert_absent("nope").await;

    // Withdrawn by its author: same.
    let again = f.propose(&f.requester, enabled("nope")).await["id"]
        .as_i64()
        .unwrap();
    assert_eq!(
        f.status(
            &f.requester,
            "DELETE",
            &f.cr_url(&f.prod, &format!("/{again}")),
            None
        )
        .await,
        204
    );
    f.assert_absent("nope").await;
    assert!(f.pending(&f.prod).await.is_empty());
}

/// 7 — losing the editor role (or the membership) takes away the right to
/// approve at once; the request stays pending for someone who still has it.
#[tokio::test]
async fn revoked_reviewer_cannot_approve_a_creation() {
    let Some(f) = fixture("cg_gov_revoke").await else {
        return;
    };
    let id = f.propose(&f.requester, enabled("guarded")).await["id"]
        .as_i64()
        .unwrap();
    let approve = f.cr_url(&f.prod, &format!("/{id}/approve"));

    let demote = f
        .status(
            &f.admin,
            "PATCH",
            &f.url(&format!(
                "/api/projects/{}/members/{}",
                f.project, f.reviewer_id
            )),
            Some(json!({ "role": "viewer" })),
        )
        .await;
    assert!((200..300).contains(&demote), "demote: {demote}");
    assert_eq!(f.status(&f.reviewer, "POST", &approve, None).await, 403);
    assert_eq!(
        f.status(
            &f.reviewer,
            "POST",
            &f.cr_url(&f.prod, &format!("/{id}/reject")),
            Some(json!({}))
        )
        .await,
        403
    );
    f.assert_absent("guarded").await;

    let remove = f
        .status(
            &f.admin,
            "DELETE",
            &f.url(&format!(
                "/api/projects/{}/members/{}",
                f.project, f.reviewer_id
            )),
            None,
        )
        .await;
    assert!((200..300).contains(&remove), "remove: {remove}");
    assert_eq!(f.status(&f.reviewer, "POST", &approve, None).await, 403);
    f.assert_absent("guarded").await;

    // Still pending: someone with the right can act on it.
    assert_eq!(f.pending(&f.prod).await.len(), 1);
    request(&f.admin, "POST", &approve, None, 200).await;
    assert_eq!(f.served("guarded").await.unwrap()["is_enabled"], true);
}

/// 8 — concurrent proposals and approvals: one pending creation per key, one
/// winner per approval, and a key taken meanwhile fails the approval cleanly.
#[tokio::test]
async fn concurrent_creation_proposals_and_approvals() {
    let Some(f) = fixture("cg_gov_concurrent").await else {
        return;
    };
    let url = f.flags_url(&f.prod);
    let send = |client: &reqwest::Client| {
        client
            .post(&url)
            .header("X-Checkgate-Request", "true")
            .json(&enabled("race"))
            .send()
    };
    let (a, b) = tokio::join!(send(&f.requester), send(&f.reviewer));
    let mut codes = [a.unwrap().status().as_u16(), b.unwrap().status().as_u16()];
    codes.sort_unstable();
    assert_eq!(codes, [202, 409], "exactly one pending creation per key");
    let pending = f.pending(&f.prod).await;
    assert_eq!(pending.len(), 1, "{pending:?}");
    let id = pending[0]["id"].as_i64().unwrap();
    let author = pending[0]["requested_by"].as_str().unwrap().to_string();
    // A retry by the same author (an operator requeueing) is also 409.
    let retry = if author == REQUESTER {
        &f.requester
    } else {
        &f.reviewer
    };
    assert_eq!(
        f.status(retry, "POST", &url, Some(enabled("race"))).await,
        409
    );

    // Two approvers at once: one applies it, the other gets 409.
    let approve_url = f.cr_url(&f.prod, &format!("/{id}/approve"));
    let other_editor = if author == REQUESTER {
        &f.reviewer
    } else {
        &f.requester
    };
    let approve = |client: &reqwest::Client| {
        client
            .post(&approve_url)
            .header("X-Checkgate-Request", "true")
            .send()
    };
    let (a, b) = tokio::join!(approve(other_editor), approve(&f.admin));
    let mut codes = [a.unwrap().status().as_u16(), b.unwrap().status().as_u16()];
    codes.sort_unstable();
    assert_eq!(codes, [200, 409]);
    let creates = f
        .audit(&f.prod, "race")
        .await
        .into_iter()
        .filter(|e| e["action"] == "CREATE")
        .count();
    assert_eq!(creates, 1, "the flag is created exactly once");
    assert_eq!(f.served("race").await.unwrap()["is_enabled"], true);

    // A key created (inert) while its creation is pending: approving the
    // request is 409, it stays pending, and the inert flag is untouched.
    let id = f.propose(&f.requester, enabled("taken")).await["id"]
        .as_i64()
        .unwrap();
    request(&f.reviewer, "POST", &url, Some(inert("taken")), 200).await;
    assert_eq!(
        f.status(
            &f.reviewer,
            "POST",
            &f.cr_url(&f.prod, &format!("/{id}/approve")),
            None
        )
        .await,
        409
    );
    assert_eq!(f.served("taken").await.unwrap()["is_enabled"], false);
    assert!(f.pending(&f.prod).await.iter().any(|c| c["id"] == id));
}

/// 9 — a member of another project can neither propose nor review here.
#[tokio::test]
async fn creation_requests_respect_project_boundaries() {
    let Some(f) = fixture("cg_gov_cross_project").await else {
        return;
    };
    assert_eq!(
        f.status(
            &f.outsider,
            "POST",
            &f.flags_url(&f.prod),
            Some(enabled("intruder"))
        )
        .await,
        403
    );
    assert!(f.pending(&f.prod).await.is_empty());
    f.assert_absent("intruder").await;

    let id = f.propose(&f.requester, enabled("ours")).await["id"]
        .as_i64()
        .unwrap();
    for (method, suffix, body) in [
        ("POST", format!("/{id}/approve"), None),
        ("POST", format!("/{id}/reject"), Some(json!({}))),
        ("DELETE", format!("/{id}"), None),
        ("GET", String::new(), None),
    ] {
        assert_eq!(
            f.status(&f.outsider, method, &f.cr_url(&f.prod, &suffix), body)
                .await,
            403,
            "{method} {suffix}"
        );
    }
    // Their own project's environment is theirs to use.
    request(
        &f.outsider,
        "POST",
        &f.flags_url(&f.foreign),
        Some(enabled("theirs")),
        200,
    )
    .await;
    f.assert_absent("ours").await;
    f.assert_absent("theirs").await;
}

/// 10 — a request belongs to its environment: it cannot be approved,
/// rejected or withdrawn through another environment's path.
#[tokio::test]
async fn creation_requests_are_bound_to_their_environment() {
    let Some(f) = fixture("cg_gov_cross_env").await else {
        return;
    };
    let id = f.propose(&f.requester, enabled("bound")).await["id"]
        .as_i64()
        .unwrap();
    for env in [&f.staging, &f.foreign] {
        for (method, suffix, body) in [
            ("POST", format!("/{id}/approve"), None),
            ("POST", format!("/{id}/reject"), Some(json!({}))),
            ("DELETE", format!("/{id}"), None),
        ] {
            let s = f
                .status(&f.admin, method, &f.cr_url(env, &suffix), body)
                .await;
            assert_eq!(s, 404, "{method} {suffix} via {env}");
        }
        assert!(f.pending(env).await.is_empty(), "listed only in Production");
    }
    f.assert_absent("bound").await;
    for env in [&f.staging, &f.foreign] {
        assert_eq!(
            f.status(&f.admin, "GET", &f.flag_url(env, "bound"), None)
                .await,
            404
        );
    }
    assert_eq!(f.pending(&f.prod).await.len(), 1);
}

/// 11 — a `propose` token creates nothing, not even a request, and cannot
/// approve someone else's creation; it may withdraw its owner's own.
#[tokio::test]
async fn propose_token_cannot_create_or_approve_creations() {
    let Some(f) = fixture("cg_gov_propose").await else {
        return;
    };
    let agent = bearer_client(&mint(&f.requester, &f.server, "propose").await);
    for env in [&f.prod, &f.staging] {
        for body in [enabled("agent-made"), inert("agent-made")] {
            assert_eq!(
                f.status(&agent, "POST", &f.flags_url(env), Some(body))
                    .await,
                403,
                "propose token create in {env}"
            );
        }
        assert!(f.pending(env).await.is_empty());
    }
    f.assert_absent("agent-made").await;

    let theirs = f.propose(&f.reviewer, enabled("reviewed")).await["id"]
        .as_i64()
        .unwrap();
    for action in ["approve", "reject"] {
        assert_eq!(
            f.status(
                &agent,
                "POST",
                &f.cr_url(&f.prod, &format!("/{theirs}/{action}")),
                Some(json!({}))
            )
            .await,
            403
        );
    }
    assert_eq!(
        f.status(
            &agent,
            "DELETE",
            &f.cr_url(&f.prod, &format!("/{theirs}")),
            None
        )
        .await,
        404,
        "not its owner's request"
    );
    f.assert_absent("reviewed").await;

    let own = f.propose(&f.requester, enabled("own")).await["id"]
        .as_i64()
        .unwrap();
    assert_eq!(
        f.status(
            &agent,
            "DELETE",
            &f.cr_url(&f.prod, &format!("/{own}")),
            None
        )
        .await,
        204
    );
    f.assert_absent("own").await;
}

/// 12 — direct API attempts around the gate.
#[tokio::test]
async fn direct_api_bypass_attempts_fail() {
    let Some(f) = fixture("cg_gov_bypass").await else {
        return;
    };
    let url = f.flags_url(&f.prod);

    // Enabled with a 0 % rollout still serves rules: queued, not stored.
    let mut zero = enabled("zero-rollout");
    zero["rollout_percentage"] = json!(0);
    zero["rules"] =
        json!([{ "attribute": "plan", "operator": "equals", "values": ["enterprise"] }]);
    f.propose(&f.requester, zero).await;
    f.assert_absent("zero-rollout").await;

    // A read_write token is its user: queued too.
    let token = bearer_client(&mint(&f.requester, &f.server, "read_write").await);
    f.propose(&token, enabled("via-token")).await;
    f.assert_absent("via-token").await;

    // read_only token and SDK key: refused outright.
    let ro = bearer_client(&mint(&f.requester, &f.server, "read_only").await);
    assert_eq!(f.status(&ro, "POST", &url, Some(enabled("ro"))).await, 403);
    assert_eq!(
        f.status(&f.prod_sdk, "POST", &url, Some(enabled("sdk")))
            .await,
        403
    );
    f.assert_absent("ro").await;
    f.assert_absent("sdk").await;

    // Overwriting an existing key by POST: 409, inert or not, nothing queued.
    request(&f.requester, "POST", &url, Some(inert("existing")), 200).await;
    for body in [enabled("existing"), inert("existing")] {
        assert_eq!(f.status(&f.requester, "POST", &url, Some(body)).await, 409);
    }
    assert_eq!(f.served("existing").await.unwrap()["is_enabled"], false);

    // Patching a flag whose creation is only pending: nothing to patch.
    assert_eq!(
        f.status(
            &f.requester,
            "PATCH",
            &f.flag_url(&f.prod, "via-token"),
            Some(json!({ "is_enabled": true }))
        )
        .await,
        404
    );

    // Promoting an enabled flag from an unprotected environment: 409.
    request(
        &f.requester,
        "POST",
        &f.flags_url(&f.staging),
        Some(enabled("promoted")),
        200,
    )
    .await;
    assert_eq!(
        f.status(
            &f.requester,
            "POST",
            &f.flag_url(&f.staging, "promoted")
                .replace("/promoted", "/promoted/promote"),
            Some(json!({ "target_env_id": f.prod }))
        )
        .await,
        409
    );
    f.assert_absent("promoted").await;

    // An invalid creation is refused up front and queues nothing.
    let mut bad = enabled("bad");
    bad["rollout_percentage"] = json!(150);
    assert_eq!(f.status(&f.requester, "POST", &url, Some(bad)).await, 422);
    let mut bad_key = enabled("x");
    bad_key["key"] = json!("has space");
    assert_eq!(
        f.status(&f.requester, "POST", &url, Some(bad_key)).await,
        422
    );
    let keys: Vec<String> = f
        .pending(&f.prod)
        .await
        .iter()
        .map(|c| c["flag_key"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(keys.len(), 2, "{keys:?}");
    assert!(keys.contains(&"zero-rollout".to_string()));
    assert!(keys.contains(&"via-token".to_string()));
}
