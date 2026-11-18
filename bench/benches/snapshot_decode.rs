#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "benchmark harness, not shipped code"
)]
//! Server-side cost of decoding stored flags for `/flags/snapshot` and the SSE
//! bootstrap: PostgreSQL hands back each `jsonb` value as JSON text, which the
//! server decodes into a `Flag`.
//!
//! - `via_value`: text → `serde_json::Value` → `Flag` (v0.1.30: `row.try_get::<Value>`
//!   then `serde_json::from_value`). Builds and frees a `BTreeMap` tree per flag.
//! - `direct`: text → `Flag` (v0.1.31: `row.try_get::<Json<Flag>>`).
//!
//! The fixture is the 101-flag set the public throughput benchmark seeds
//! (bench/public/sdk/fixtures.mjs `serverFlags()`), as PostgreSQL returns it.

use checkgate_core::evaluator::Flag;
use criterion::{Criterion, criterion_group, criterion_main};
use serde_json::{Value, json};
use std::hint::black_box;

fn server_flags() -> Vec<String> {
    let mut flags: Vec<Value> = (0..100)
        .map(|i| {
            let key = format!("flag-{i}");
            match i % 4 {
                0 => json!({"key": key, "is_enabled": true, "rollout_percentage": null, "description": null, "flag_type": "boolean", "rules": []}),
                1 => json!({"key": key, "is_enabled": true, "rollout_percentage": 25, "description": null, "flag_type": "boolean", "rules": []}),
                2 => json!({"key": key, "is_enabled": true, "rollout_percentage": null, "description": null, "flag_type": "boolean",
                    "rules": [{"attribute": "email", "operator": "ends_with", "values": ["@acme.test"]}]}),
                _ => json!({"key": key, "is_enabled": true, "rollout_percentage": null, "description": null, "flag_type": "string",
                    "default_value": "a", "variants": [{"weight": 50, "value": "a"}, {"weight": 50, "value": "b"}], "rules": []}),
            }
        })
        .collect();
    flags.push(json!({"key": "hot", "is_enabled": true, "rollout_percentage": null, "description": null, "flag_type": "string", "default_value": "u0", "rules": []}));
    flags.iter().map(Value::to_string).collect()
}

fn decode(c: &mut Criterion) {
    let rows = server_flags();
    // Both paths must agree before either is timed.
    for row in &rows {
        let a: Flag = serde_json::from_value(serde_json::from_str::<Value>(row).unwrap()).unwrap();
        let b: Flag = serde_json::from_str(row).unwrap();
        assert_eq!(a, b);
    }
    let mut group = c.benchmark_group("snapshot_decode_101_flags");
    group.bench_function("via_value", |b| {
        b.iter(|| {
            for row in &rows {
                let v: Value = serde_json::from_str(black_box(row)).unwrap();
                black_box(serde_json::from_value::<Flag>(v).unwrap());
            }
        });
    });
    group.bench_function("direct", |b| {
        b.iter(|| {
            for row in &rows {
                black_box(serde_json::from_str::<Flag>(black_box(row)).unwrap());
            }
        });
    });
    group.finish();
}

criterion_group!(benches, decode);
criterion_main!(benches);
