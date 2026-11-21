#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test code: a panic here is an assertion failure, not a DoS"
)]
//! The shared evaluation vectors (`sdks/conformance/vectors/evaluation.json`).
//!
//! The Rust core is the oracle: this test checks that the file's `expect`
//! entries are what the core computes, and every SDK checks itself against
//! the same file. After a deliberate semantic change, regenerate with
//! `CHECKGATE_BLESS_VECTORS=1 cargo test -p checkgate-core --test vectors`
//! and review the diff: every changed line is a behaviour change users see.

use checkgate_core::evaluator::{evaluate_variant, explain, Flag, UserContext};
use checkgate_core::store::FlagStore;
use serde_json::{json, Value};
use std::path::PathBuf;

fn vectors_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../sdks/conformance/vectors/evaluation.json")
}

fn store(doc: &Value) -> FlagStore {
    let store = FlagStore::new();
    for flag in doc["flags"].as_array().unwrap() {
        let parsed: Flag = serde_json::from_value(flag.clone())
            .unwrap_or_else(|e| panic!("vector flag {} must parse: {e}", flag["key"]));
        store.upsert_flag(parsed);
    }
    store
}

fn context(case: &Value) -> UserContext {
    UserContext {
        key: case["user"].as_str().unwrap().to_owned(),
        attributes: case["attributes"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.as_str()
                        .expect("vector attributes are strings")
                        .to_owned(),
                )
            })
            .collect(),
    }
}

/// What every SDK must reproduce for one case, or `null` for a missing flag.
fn expected(store: &FlagStore, case: &Value) -> Value {
    let Some(flag) = store.get_flag(case["flag"].as_str().unwrap()) else {
        return Value::Null;
    };
    let ctx = context(case);
    let explanation = explain(&flag, &ctx, store);
    // The explanation must be the plain evaluation, plus why.
    assert_eq!(
        explanation.result(),
        evaluate_variant(&flag, &ctx, store),
        "explain and evaluate_variant disagree on {}",
        case["name"]
    );
    let mut out = json!({
        "enabled": explanation.enabled,
        "value": explanation.value,
        "reason": explanation.reason,
    });
    let obj = out.as_object_mut().unwrap();
    if let Some(i) = explanation.rule_index {
        obj.insert("rule_index".into(), json!(i));
    }
    if let Some(b) = explanation.rollout_bucket {
        obj.insert("rollout_bucket".into(), json!(b));
    }
    if let Some(i) = explanation.variant_index {
        obj.insert("variant_index".into(), json!(i));
    }
    out
}

#[test]
fn vectors_match_the_core() {
    let raw = std::fs::read_to_string(vectors_path()).expect("vectors file");
    let mut doc: Value = serde_json::from_str(&raw).expect("vectors are JSON");
    let store = store(&doc);
    let bless = std::env::var_os("CHECKGATE_BLESS_VECTORS").is_some();

    let mut mismatches = Vec::new();
    let mut names = std::collections::HashSet::new();
    for case in doc["cases"].as_array_mut().unwrap() {
        assert!(
            names.insert(case["name"].as_str().unwrap().to_owned()),
            "duplicate case name {}",
            case["name"]
        );
        let want = expected(&store, case);
        if bless {
            case["expect"] = want;
        } else if case.get("expect") != Some(&want) {
            mismatches.push(format!(
                "{}: file says {}, core computes {want}",
                case["name"],
                case.get("expect").unwrap_or(&Value::Null)
            ));
        }
    }

    for bad in doc["malformed"].as_array().unwrap() {
        let json = bad["json"].as_str().unwrap();
        assert!(
            serde_json::from_str::<Flag>(json).is_err(),
            "malformed vector must be rejected: {json}"
        );
    }

    if bless {
        let mut text = serde_json::to_string_pretty(&doc).unwrap();
        text.push('\n');
        std::fs::write(vectors_path(), text).unwrap();
        return;
    }
    assert!(
        mismatches.is_empty(),
        "{} vector(s) disagree with the core (bless after a deliberate change):\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

#[test]
fn every_reason_is_covered() {
    // A new reason without a vector would let SDKs drift on it unnoticed.
    let raw = std::fs::read_to_string(vectors_path()).unwrap();
    let doc: Value = serde_json::from_str(&raw).unwrap();
    let seen: std::collections::HashSet<&str> = doc["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["expect"]["reason"].as_str())
        .collect();
    for reason in [
        "DISABLED",
        "PREREQUISITE_FAILED",
        "TARGETING_MATCH",
        "ROLLOUT_EXCLUDED",
        "SPLIT",
        "DEFAULT",
        "STATIC",
    ] {
        assert!(seen.contains(reason), "no vector produces {reason}");
    }
}
