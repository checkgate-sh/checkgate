#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::print_stdout,
    reason = "release benchmark harness, not shipped code"
)]
//! Release A/B harness for the Rust evaluation core
//! (scripts/release-benchmarks/v0.2.0-linux.sh).
//!
//! The same file is copied into the baseline and the candidate worktrees as
//! `bench/benches/release_core_ab.rs`, so both builds run an identical
//! workload. It uses only API both releases have (`FlagStore::new`,
//! `upsert_flag`, `get_flag`, `evaluate_variant`) and builds flags from JSON,
//! so it compiles against v0.1.31 and v0.2.0 alike. No Criterion: each
//! release's own Criterion suite differs, and this harness must not.
//!
//! Method, per case: `--warmup` evaluations, then `--samples` batches of
//! `--batch` evaluations each, timed with `Instant`. A sample is a batch mean
//! (ns per evaluation); the percentiles describe the spread of batch means.
//! User keys cycle through 1,024 precomputed contexts so bucketing and rule
//! matching are not one cached path. Output: one JSON document on stdout.

use checkgate_core::evaluator::{Flag, UserContext, evaluate_variant};
use checkgate_core::store::FlagStore;
use serde_json::{Value, json};
use std::hint::black_box;
use std::time::Instant;

fn flag(v: Value) -> Flag {
    serde_json::from_value(v).expect("flag JSON valid in both releases")
}

fn boolean(key: &str) -> Flag {
    flag(json!({ "key": key, "is_enabled": true, "rollout_percentage": null,
                 "description": null, "rules": [] }))
}

/// `n` rules on distinct attributes; only the last one matches the users.
fn targeted(key: &str, n: usize) -> Flag {
    let mut rules: Vec<Value> = (0..n.saturating_sub(1))
        .map(|i| json!({ "attribute": format!("attr_{i}"), "operator": "equals",
                         "values": ["no-match-a", "no-match-b"] }))
        .collect();
    rules.push(json!({ "attribute": "plan", "operator": "equals", "values": ["free", "pro"] }));
    flag(json!({ "key": key, "is_enabled": true, "rollout_percentage": null,
                 "description": null, "rules": rules }))
}

fn rollout(key: &str) -> Flag {
    flag(json!({ "key": key, "is_enabled": true, "rollout_percentage": 50,
                 "description": null, "rules": [] }))
}

fn multivariate(key: &str) -> Flag {
    flag(json!({ "key": key, "is_enabled": true, "rollout_percentage": null,
                 "description": null, "rules": [], "flag_type": "string",
                 "default_value": "control",
                 "variants": [ { "weight": 60, "value": "control" },
                               { "weight": 30, "value": "blue" },
                               { "weight": 10, "value": "green" } ] }))
}

/// `depth` flags: `<prefix>-0` requires `<prefix>-1` ... requires the last.
fn chain(store: &FlagStore, prefix: &str, depth: usize) -> String {
    for i in 0..depth {
        let prerequisites: Vec<Value> = if i + 1 < depth {
            vec![json!({ "flag_key": format!("{prefix}-{}", i + 1) })]
        } else {
            Vec::new()
        };
        store.upsert_flag(flag(json!({
            "key": format!("{prefix}-{i}"), "is_enabled": true, "rollout_percentage": null,
            "description": null, "rules": [], "prerequisites": prerequisites })));
    }
    format!("{prefix}-0")
}

/// A store of `n` flags of mixed kinds, like a real environment: mostly
/// booleans, every 10th targeted, every 7th a rollout, every 13th weighted.
fn store_with(n: usize) -> (FlagStore, Vec<String>) {
    let store = FlagStore::new();
    let mut keys = Vec::with_capacity(n);
    for i in 0..n {
        let key = format!("flag-{i}");
        let f = if i % 13 == 0 {
            multivariate(&key)
        } else if i % 10 == 0 {
            targeted(&key, 3)
        } else if i % 7 == 0 {
            rollout(&key)
        } else {
            boolean(&key)
        };
        store.upsert_flag(f);
        keys.push(key);
    }
    (store, keys)
}

fn users() -> Vec<UserContext> {
    (0..1024)
        .map(|i| {
            serde_json::from_value(json!({
                "key": format!("user-{i:05}"),
                "attributes": { "plan": if i % 2 == 0 { "pro" } else { "free" },
                                "country": "NL", "app_version": "5.2.0" }
            }))
            .unwrap()
        })
        .collect()
}

struct Args {
    samples: usize,
    batch: usize,
    warmup: usize,
    cases: Option<Vec<String>>,
}

fn args() -> Args {
    let mut a = Args {
        samples: 60,
        batch: 20_000,
        warmup: 200_000,
        cases: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        // `cargo bench` passes `--bench`; ignore anything we do not know.
        let mut v = || it.next().expect("flag needs a value");
        match k.as_str() {
            "--samples" => a.samples = v().parse().unwrap(),
            "--batch" => a.batch = v().parse().unwrap(),
            "--warmup" => a.warmup = v().parse().unwrap(),
            "--cases" => a.cases = Some(v().split(',').map(str::to_string).collect()),
            _ => {}
        }
    }
    a
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// Runs `f(i)` (one evaluation, or one sweep for `per_call` > 1) and returns
/// ns per evaluation for each sample.
fn measure(a: &Args, per_call: usize, mut f: impl FnMut(usize)) -> Value {
    let calls = |n: usize| n.div_ceil(per_call).max(1);
    for i in 0..calls(a.warmup) {
        f(i);
    }
    let batch_calls = calls(a.batch);
    let mut samples = Vec::with_capacity(a.samples);
    for s in 0..a.samples {
        let start = Instant::now();
        for i in 0..batch_calls {
            f(s * batch_calls + i);
        }
        let ns = start.elapsed().as_nanos() as f64;
        samples.push(ns / (batch_calls * per_call) as f64);
    }
    let mut sorted = samples.clone();
    sorted.sort_by(f64::total_cmp);
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    let var = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / samples.len() as f64;
    let r3 = |x: f64| (x * 1000.0).round() / 1000.0;
    json!({
        "unit": "ns_per_evaluation",
        "evaluations_per_sample": batch_calls * per_call,
        "samples_ns": samples.iter().map(|x| r3(*x)).collect::<Vec<_>>(),
        "median_ns": r3(pct(&sorted, 0.5)),
        "p95_ns": r3(pct(&sorted, 0.95)),
        "p99_ns": r3(pct(&sorted, 0.99)),
        "mean_ns": r3(mean),
        "cv": r3(var.sqrt() / mean),
    })
}

fn main() {
    let a = args();
    let ctx = users();
    let u = |i: usize| &ctx[i % ctx.len()];

    let (store, _) = store_with(1_000);
    store.upsert_flag(targeted("t1", 1));
    store.upsert_flag(targeted("t10", 10));
    store.upsert_flag(rollout("rollout"));
    store.upsert_flag(multivariate("mv"));
    let head = chain(&store, "chain", 5);
    let (big, big_keys) = store_with(10_000);
    let (sweep_store, sweep_keys) = store_with(1_000);

    let eval = |s: &FlagStore, key: &str, user: &UserContext| {
        let f = s.get_flag(black_box(key)).unwrap();
        black_box(evaluate_variant(&f, black_box(user), s));
    };

    let mut out = serde_json::Map::new();
    let want = |name: &str| a.cases.as_ref().is_none_or(|c| c.iter().any(|x| x == name));
    let mut run = |name: &str, per_call: usize, f: &mut dyn FnMut(usize)| {
        if want(name) {
            eprintln!("case {name}");
            out.insert(name.to_string(), measure(&a, per_call, f));
        }
    };
    run("boolean", 1, &mut |i| eval(&store, "flag-501", u(i)));
    run("targeting_1_rule", 1, &mut |i| eval(&store, "t1", u(i)));
    run("targeting_10_rules", 1, &mut |i| eval(&store, "t10", u(i)));
    run("percentage_rollout", 1, &mut |i| eval(&store, "rollout", u(i)));
    run("weighted_multivariate", 1, &mut |i| eval(&store, "mv", u(i)));
    run("prerequisite_chain_5", 1, &mut |i| eval(&store, &head, u(i)));
    // Large flag sets: one lookup + evaluation in a 10,000-flag store, and
    // every flag of a 1,000-flag store for one user (an "all flags" call);
    // the sweep is reported per flag.
    run("lookup_10000_flags", 1, &mut |i| {
        eval(&big, &big_keys[(i * 7919) % big_keys.len()], u(i));
    });
    run("all_flags_1000", sweep_keys.len(), &mut |i| {
        let user = u(i);
        for k in &sweep_keys {
            eval(&sweep_store, k, user);
        }
    });

    let doc = json!({
        "harness": "release_core_ab",
        "samples": a.samples,
        "batch": a.batch,
        "warmup": a.warmup,
        "cases": Value::Object(out),
    });
    println!("{doc}");
}
