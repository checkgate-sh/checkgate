#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! SSE liveness, credential and randomized fault tests against real server
//! processes, PostgreSQL and Redis (v0.1.31 RC2).
//!
//! Clients are `common::sdk::SimSdk`, which follows the shipped SDKs' rules,
//! including the connect and idle deadlines and terminal 401/403. Faults sit
//! between client and server (`ChaosProxy` links), between servers and Redis,
//! and inside PostgreSQL (a table made unreadable after a commit).
//!
//! The randomized test takes its seeds from `CHECKGATE_FAULT_SEEDS`
//! ("1,2,3" or "1-50"; default "1,2,3") and its step count from
//! `CHECKGATE_FAULT_STEPS` (default 25). A failure names the seed and prints
//! the steps; rerun with `CHECKGATE_FAULT_SEEDS=<seed>`. The generator is a
//! SplitMix64 written out here, so a seed means the same steps on every
//! machine and across dependency upgrades.
//!
//! Requires CHECKGATE_TEST_DATABASE_URL / CHECKGATE_TEST_REDIS_URL.

mod common;

use checkgate_core::evaluator::{Flag, UserContext, evaluate};
use checkgate_core::store::FlagStore;
use common::chaos::ChaosProxy;
use common::cluster::Cluster;
use common::sdk::{SdkOptions, SimSdk};
use common::{request, test_env};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The server heartbeat in these tests: an idle deadline of 3 s.
const HEARTBEAT: &[(&str, Option<&str>)] = &[("SSE_HEARTBEAT_INTERVAL_SECS", Some("1"))];
const CONVERGE: Duration = Duration::from_secs(20);

fn opts() -> SdkOptions {
    SdkOptions {
        connect_timeout: Duration::from_secs(3),
        ..SdkOptions::default()
    }
}

/// SplitMix64: tiny, fixed forever, good enough to pick test steps.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

fn seeds() -> Vec<u64> {
    let raw = std::env::var("CHECKGATE_FAULT_SEEDS").unwrap_or_else(|_| "1,2,3".into());
    raw.split(',')
        .flat_map(|part| match part.split_once('-') {
            Some((a, b)) => {
                (a.trim().parse::<u64>().unwrap()..=b.trim().parse().unwrap()).collect()
            }
            None => vec![part.trim().parse().unwrap()],
        })
        .collect()
}

fn evaluate_for(flag: &Value, user: &str, attrs: &[(&str, &str)]) -> bool {
    let flag: Flag = serde_json::from_value(flag.clone()).unwrap();
    let store = FlagStore::new();
    store.upsert_flag(flag.clone());
    let ctx = UserContext {
        key: user.into(),
        attributes: attrs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
    };
    evaluate(&flag, &ctx, &store)
}

async fn wait_until(what: &str, within: Duration, mut ok: impl FnMut() -> bool) -> Duration {
    let start = Instant::now();
    while !ok() {
        assert!(start.elapsed() < within, "{what}: not within {within:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    start.elapsed()
}

// ── Liveness ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_quiet_stream_gets_heartbeats_and_is_never_replaced() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(&env, "fault_heartbeat", 1, HEARTBEAT).await;
    c.create_flag(0, "quiet", true).await;
    let sdk = SimSdk::start_with("sdk", &c.base(0), &c.sdk_key, opts());
    c.wait_converged(&[&sdk], &c.env_id, CONVERGE).await;

    // Five idle deadlines with no flag change at all.
    tokio::time::sleep(Duration::from_secs(15)).await;
    sdk.with(|s| {
        assert_eq!(
            s.heartbeat_ms,
            Some(1000),
            "connected announces the interval"
        );
        assert!(
            s.heartbeats >= 10,
            "heartbeats on a quiet stream: {}",
            s.heartbeats
        );
        assert_eq!(s.attempts, 1, "a healthy quiet stream is never replaced");
        assert_eq!(s.deadline_trips, 0);
    });
}

#[tokio::test]
async fn a_silently_dead_stream_is_replaced_within_the_idle_deadline() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(&env, "fault_dead_stream", 1, HEARTBEAT).await;
    c.create_flag(0, "dead", false).await;
    let link = ChaosProxy::start(&format!("127.0.0.1:{}", c.instances[0].port)).await;
    let sdk = SimSdk::start_with(
        "sdk",
        &format!("http://127.0.0.1:{}", link.port),
        &c.sdk_key,
        opts(),
    );
    c.wait_converged(&[&sdk], &c.env_id, CONVERGE).await;

    for round in 0..3 {
        // A middlebox forgets the connection: no FIN, no RST, no bytes, ever.
        link.strand_existing();
        let enabled = round % 2 == 0;
        c.set_enabled(0, "dead", enabled).await;
        let t = c.wait_converged(&[&sdk], &c.env_id, CONVERGE).await;
        c.record("stranded client stream (1 s heartbeat)", t);
        // 3 s idle deadline + backoff (first retry 200 ms + 30 %) + bootstrap.
        assert!(
            t < Duration::from_secs(6),
            "round {round}: recovered after {t:?}"
        );
    }
    sdk.with(|s| assert!(s.deadline_trips >= 3, "trips: {}", s.deadline_trips));
}

// ── Credentials ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_revoked_key_is_terminal_and_a_restored_key_works_again() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(&env, "fault_revoked_key", 1, HEARTBEAT).await;
    c.create_flag(0, "auth", true).await;
    let link = ChaosProxy::start(&format!("127.0.0.1:{}", c.instances[0].port)).await;
    let base = format!("http://127.0.0.1:{}", link.port);
    let sdk = SimSdk::start_with("sdk", &base, &c.sdk_key, opts());
    c.wait_converged(&[&sdk], &c.env_id, CONVERGE).await;

    // Revoke: the key is checked against PostgreSQL on every request, on
    // every replica. The open stream survives (documented, N3); drop it.
    sqlx::query("UPDATE sdk_keys SET value = value || '-revoked' WHERE value = $1")
        .bind(&c.sdk_key)
        .execute(&c.pool)
        .await
        .unwrap();
    link.sever();
    wait_until("401 seen", Duration::from_secs(10), || {
        sdk.with(|s| s.auth_rejected.is_some())
    })
    .await;
    let attempts = sdk.with(|s| s.attempts);
    tokio::time::sleep(Duration::from_secs(3)).await;
    sdk.with(|s| {
        assert_eq!(s.auth_rejected, Some(401));
        assert_eq!(s.attempts, attempts, "no retry with a rejected key");
        assert!(!s.polling);
        assert!(s.flags.contains_key("auth"), "last known flags are kept");
    });

    sqlx::query("UPDATE sdk_keys SET value = $1 WHERE value = $1 || '-revoked'")
        .bind(&c.sdk_key)
        .execute(&c.pool)
        .await
        .unwrap();
    let again = SimSdk::start_with("sdk-again", &base, &c.sdk_key, opts());
    c.wait_converged(&[&again], &c.env_id, CONVERGE).await;
}

// ── Randomized faults ────────────────────────────────────────────────────────

/// Per client and flag: the last revision seen on the stream, and the
/// evaluation made at it, for the regression and flapping checks.
type Seen = HashMap<(String, String), (i64, Option<bool>)>;

fn check(sdks: &[SimSdk], seen: &Mutex<Seen>, violations: &Mutex<Vec<String>>) {
    let mut seen = seen.lock().unwrap();
    for sdk in sdks {
        let (flags, revisions) = sdk.snapshot();
        for (key, known) in &revisions {
            if known.deleted {
                continue;
            }
            let id = (sdk.name.clone(), key.clone());
            let value = flags.get(key).map(|f| evaluate_for(f, "user-7", &[]));
            if let Some((rev, last)) = seen.get(&id) {
                if known.revision < *rev {
                    violations.lock().unwrap().push(format!(
                        "{}: {key} went back from rev {rev} to {}",
                        sdk.name, known.revision
                    ));
                }
                if known.revision == *rev && value.is_some() && last.is_some() && value != *last {
                    violations.lock().unwrap().push(format!(
                        "{}: {key} flapped {last:?} -> {value:?} at unchanged rev {rev}",
                        sdk.name
                    ));
                }
            }
            seen.insert(id, (known.revision, value));
        }
    }
}

#[tokio::test]
async fn randomized_faults_converge_without_regression_or_flapping() {
    let Some(env) = test_env() else { return };
    let steps: usize = std::env::var("CHECKGATE_FAULT_STEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(25);
    for seed in seeds() {
        run_seed(&env, seed, steps).await;
    }
}

async fn run_seed(env: &common::TestEnv, seed: u64, steps: usize) {
    let mut rng = Rng(seed);
    let mut c = Cluster::start(env, &format!("fault_seed_{seed}"), 2, HEARTBEAT).await;
    let keys = ["k0", "k1", "k2", "k3"];
    for key in &keys[..2] {
        c.create_flag(0, key, true).await;
    }
    let links = [
        ChaosProxy::start(&format!("127.0.0.1:{}", c.instances[0].port)).await,
        ChaosProxy::start(&format!("127.0.0.1:{}", c.instances[1].port)).await,
    ];
    let sdks = Arc::new(vec![
        SimSdk::start_with(
            "via-link-0",
            &format!("http://127.0.0.1:{}", links[0].port),
            &c.sdk_key,
            opts(),
        ),
        SimSdk::start_with(
            "via-link-1",
            &format!("http://127.0.0.1:{}", links[1].port),
            &c.sdk_key,
            opts(),
        ),
        SimSdk::start_with("direct-1", &c.base(1), &c.sdk_key, opts()),
    ]);
    {
        let refs: Vec<&SimSdk> = sdks.iter().collect();
        c.wait_converged(&refs, &c.env_id, CONVERGE).await;
    }

    let seen = Arc::new(Mutex::new(Seen::new()));
    let violations = Arc::new(Mutex::new(Vec::<String>::new()));
    let sampler = {
        let (sdks, seen, violations) = (
            Arc::clone(&sdks),
            Arc::clone(&seen),
            Arc::clone(&violations),
        );
        tokio::spawn(async move {
            loop {
                check(&sdks, &seen, &violations);
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
    };

    let mut trace = Vec::new();
    let mut killed: Option<usize> = None;
    for _ in 0..steps {
        let i = usize::try_from(rng.below(2)).unwrap();
        // A write must reach a live instance; any instance otherwise.
        let w = killed.map_or(i, |k| 1 - k);
        let key = keys[usize::try_from(rng.below(4)).unwrap()];
        let step = match rng.below(100) {
            0..=24 => {
                let enabled = rng.chance(50);
                let code = c.try_set_enabled(w, key, enabled).await;
                if code == 404 {
                    let _ = c.try_create(w, key, enabled).await;
                }
                format!("set {key}={enabled} via {w} ({code})")
            }
            25..=34 => {
                let pct = rng.below(101);
                let url = format!("{}/api/environments/{}/flags/{key}", c.base(w), c.env_id);
                let code = c
                    .admin
                    .patch(url)
                    .header("X-Checkgate-Request", "true")
                    .json(&json!({ "rollout_percentage": pct }))
                    .send()
                    .await
                    .map(|r| r.status().as_u16())
                    .unwrap_or(0);
                format!("rollout {key}={pct}% via {w} ({code})")
            }
            35..=42 => format!("delete {key} via {w} ({})", c.try_delete(w, key).await),
            43..=52 => {
                links[i].sever();
                format!("sever link {i}")
            }
            53..=60 => {
                links[i].strand_existing();
                format!("strand link {i}")
            }
            61..=66 => {
                links[i].refuse();
                tokio::time::sleep(Duration::from_millis(300 + rng.below(700))).await;
                links[i].heal();
                format!("refuse link {i} briefly")
            }
            67..=71 => {
                links[i].blackhole();
                tokio::time::sleep(Duration::from_millis(500 + rng.below(3000))).await;
                links[i].heal();
                format!("blackhole link {i}")
            }
            72..=77 => {
                c.redis(i).refuse();
                tokio::time::sleep(Duration::from_millis(300 + rng.below(1500))).await;
                c.redis(i).heal();
                format!("redis down for instance {i}")
            }
            78..=81 if killed.is_none() => {
                c.kill(i);
                killed = Some(i);
                format!("kill instance {i}")
            }
            78..=81 => {
                let k = killed.take().unwrap();
                c.start_instance(k).await;
                format!("restart instance {k}")
            }
            _ => {
                let ms = 50 + rng.below(800);
                tokio::time::sleep(Duration::from_millis(ms)).await;
                format!("wait {ms} ms")
            }
        };
        trace.push(step);
    }

    // Heal everything, then every client must hold the committed state.
    for link in &links {
        link.heal();
    }
    for i in 0..2 {
        c.redis(i).heal();
    }
    if let Some(k) = killed.take() {
        c.start_instance(k).await;
        trace.push(format!("restart instance {k}"));
    }
    let healed = Instant::now();
    let converged = {
        let refs: Vec<&SimSdk> = sdks.iter().collect();
        let truth_ok = tokio::time::timeout(CONVERGE, async {
            loop {
                let truth = c.authoritative(&c.env_id).await;
                if refs
                    .iter()
                    .all(|s| common::cluster::divergence(s, &truth).is_none())
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
        truth_ok.is_ok()
    };
    sampler.abort();
    check(&sdks, &seen, &violations);
    let elapsed = healed.elapsed();
    let violations = violations.lock().unwrap().clone();
    let stale = if converged {
        None
    } else {
        let truth = c.authoritative(&c.env_id).await;
        Some(
            sdks.iter()
                .filter_map(|s| {
                    common::cluster::divergence(s, &truth).map(|d| format!("{}: {d}", s.name))
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    };
    assert!(
        converged && violations.is_empty(),
        "seed {seed}: converged={converged} after {elapsed:?}\nstale: {stale:?}\nviolations: {violations:#?}\nsteps:\n  {}",
        trace.join("\n  ")
    );
    c.record("randomized faults: convergence after healing", elapsed);
    eprintln!(
        "SEED {seed}: {} steps, converged {elapsed:?} after healing",
        trace.len()
    );
}

// ── C1: segment read failure after commit ────────────────────────────────────

/// The rule a flag gets from segment `staff` once expanded.
fn has_staff_rule(flag: &Value) -> bool {
    flag["rules"].as_array().is_some_and(|rules| {
        rules
            .iter()
            .any(|r| r["attribute"] == "email" && r["operator"] == "ends_with")
    })
}

#[tokio::test]
async fn a_segment_read_failure_after_commit_publishes_nothing_stale_and_recovers() {
    let Some(env) = test_env() else { return };
    let c = Cluster::start(&env, "fault_segment_read", 2, HEARTBEAT).await;
    request(
        &c.admin,
        "POST",
        &format!("{}/api/environments/{}/segments", c.base(0), c.env_id),
        Some(json!({
            "name": "Staff", "key": "staff",
            "rules": [{ "attribute": "email", "operator": "ends_with", "values": ["@acme.test"] }]
        })),
        200,
    )
    .await;
    c.create_flag_body(
        0,
        &c.env_id,
        json!({ "key": "beta", "is_enabled": true, "flag_type": "boolean",
                "rollout_percentage": 0, "rules": [{ "segment_key": "staff" }] }),
    )
    .await;
    let sdks = [
        SimSdk::start_with("on-writer", &c.base(0), &c.sdk_key, opts()),
        SimSdk::start_with("on-other", &c.base(1), &c.sdk_key, opts()),
    ];
    c.wait_converged(&[&sdks[0], &sdks[1]], &c.env_id, CONVERGE)
        .await;
    for sdk in &sdks {
        let (flags, _) = sdk.snapshot();
        assert!(
            has_staff_rule(&flags["beta"]),
            "{}: segment expanded",
            sdk.name
        );
    }
    let staff = [("email", "ann@acme.test")];
    let before_rev = c.authoritative(&c.env_id).await["beta"].0;

    // The fault: after the next commit, the segments cannot be read at all.
    // Nothing on the flag write path reads them, so the write commits.
    sqlx::query("ALTER TABLE segments RENAME TO segments_unreadable")
        .execute(&c.pool)
        .await
        .unwrap();
    let url = format!("{}/api/environments/{}/flags/beta", c.base(0), c.env_id);
    request(
        &c.admin,
        "PATCH",
        &url,
        Some(json!({ "rollout_percentage": 50 })),
        200,
    )
    .await;
    let new_rev = c.authoritative(&c.env_id).await["beta"].0;
    assert!(new_rev > before_rev, "the change committed");

    // While segments stay unreadable: no client may ever hold the new
    // revision without its segment rules, and staff keep their access.
    let window = Instant::now();
    while window.elapsed() < Duration::from_secs(4) {
        for sdk in &sdks {
            let (flags, revisions) = sdk.snapshot();
            if let Some(flag) = flags.get("beta") {
                assert!(
                    has_staff_rule(flag),
                    "{}: serves beta without its segment rules (rev {:?}): {flag}",
                    sdk.name,
                    revisions.get("beta")
                );
                assert!(
                    evaluate_for(flag, "ann", &staff),
                    "{}: staff lost access",
                    sdk.name
                );
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The snapshot fails rather than answer with stripped flags.
    let snapshot = reqwest::Client::new()
        .get(format!("{}/flags/snapshot", c.base(1)))
        .bearer_auth(&c.sdk_key)
        .send()
        .await
        .unwrap();
    assert_eq!(snapshot.status(), 500, "snapshot during the fault");
    // The writer took the no-publish branch (and counted it as
    // checkgate_resync_requests_total{trigger="segment_read_failed"}).
    assert!(
        c.logs()
            .iter()
            .any(|(_, log)| log
                .contains("Could not read segments to expand a committed flag change")),
        "the writer skipped the publish because the segments were unreadable"
    );

    // Recovery: segments readable again, every client converges to the new
    // revision with the segment expanded, and so does the snapshot.
    sqlx::query("ALTER TABLE segments_unreadable RENAME TO segments")
        .execute(&c.pool)
        .await
        .unwrap();
    let t = c
        .wait_converged(&[&sdks[0], &sdks[1]], &c.env_id, CONVERGE)
        .await;
    c.record("segment read failure: recovery after segments readable", t);
    for sdk in &sdks {
        let (flags, revisions) = sdk.snapshot();
        let beta = &flags["beta"];
        assert!(
            has_staff_rule(beta),
            "{}: expanded after recovery",
            sdk.name
        );
        assert_eq!(beta["rollout_percentage"], 50);
        if let Some(k) = revisions.get("beta") {
            assert_eq!(k.revision, new_rev);
        }
        assert!(evaluate_for(beta, "ann", &staff));
    }
    let flags: Vec<Value> = reqwest::Client::new()
        .get(format!("{}/flags/snapshot", c.base(1)))
        .bearer_auth(&c.sdk_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let beta = flags.iter().find(|f| f["key"] == "beta").unwrap();
    assert!(has_staff_rule(beta) && beta["rollout_percentage"] == 50);
}
