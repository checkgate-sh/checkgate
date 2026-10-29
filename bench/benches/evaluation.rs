#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    reason = "benchmark harness, not shipped code"
)]
//! Benchmarks of the Rust evaluation core shared by every SDK.
//!
//! These measure `checkgate-core` alone: a `FlagStore` lookup plus evaluation
//! in-process. They do not include the SDK binding layers (N-API, wasm-bindgen,
//! JSI, FFI), whose call overhead is separate and usually larger.
//!
//!     cargo bench -p checkgate-bench --bench evaluation
//!
//! Before the Criterion groups run, an allocation profile (allocations per
//! evaluation, retained bytes per stored flag) is printed as JSON lines,
//! measured with a counting global allocator.

use checkgate_core::evaluator::{Flag, UserContext, evaluate, evaluate_variant};
use checkgate_core::store::FlagStore;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group};
use serde_json::json;
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

// ── Allocation counting ─────────────────────────────────────────────────────

struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is forwarded unchanged to the system allocator; the
// counters are plain atomics and never affect the returned memory.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        LIVE_BYTES.fetch_add(new_size, Ordering::Relaxed);
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: same contract as the caller's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

// ── Fixtures ────────────────────────────────────────────────────────────────

const SIZES: [usize; 4] = [10, 100, 1_000, 10_000];

fn flag(v: serde_json::Value) -> Flag {
    serde_json::from_value(v).unwrap()
}

fn boolean(key: &str) -> Flag {
    flag(json!({ "key": key, "is_enabled": true, "flag_type": "boolean", "rules": [] }))
}

/// `n` rules that do not match the benchmark user, then one that does.
fn targeted(key: &str, rules: usize) -> Flag {
    let mut r: Vec<_> = (0..rules.saturating_sub(1))
        .map(|i| json!({ "attribute": "email", "operator": "ends_with", "values": [format!("@other{i}.test")] }))
        .collect();
    r.push(json!({ "attribute": "plan", "operator": "equals", "values": ["enterprise"], "variant": true }));
    flag(json!({ "key": key, "is_enabled": true, "flag_type": "boolean", "rules": r }))
}

fn rollout(key: &str) -> Flag {
    flag(
        json!({ "key": key, "is_enabled": true, "rollout_percentage": 50, "flag_type": "boolean", "rules": [] }),
    )
}

fn multivariate(key: &str) -> Flag {
    flag(json!({
        "key": key, "is_enabled": true, "flag_type": "string", "rules": [],
        "default_value": "control", "disabled_value": "off",
        "variants": [
            { "weight": 60, "value": "a" }, { "weight": 30, "value": "b" }, { "weight": 10, "value": "c" }
        ]
    }))
}

/// `depth` flags where each requires the next; returns the head's key.
fn chain(store: &FlagStore, depth: usize) -> String {
    for i in 0..depth {
        let prereqs = if i + 1 < depth {
            json!([{ "flag_key": format!("chain-{}", i + 1) }])
        } else {
            json!([])
        };
        store.upsert_flag(flag(json!({
            "key": format!("chain-{i}"), "is_enabled": true, "flag_type": "boolean",
            "rules": [], "prerequisites": prereqs
        })));
    }
    "chain-0".to_string()
}

fn store_with(n: usize) -> FlagStore {
    let store = FlagStore::new();
    for i in 0..n {
        store.upsert_flag(boolean(&format!("flag-{i}")));
    }
    store
}

/// A representative user: stable key plus a handful of attributes.
fn user() -> UserContext {
    UserContext {
        key: "user-8f14e45f-ceea-467f-a0e6-5e7d1c2b9a31".into(),
        attributes: HashMap::from([
            ("email".into(), "dana@example.test".into()),
            ("plan".into(), "enterprise".into()),
            ("country".into(), "PH".into()),
            ("app_version".into(), "4.12.0".into()),
            ("platform".into(), "android".into()),
        ]),
    }
}

fn eval_key(store: &FlagStore, key: &str, ctx: &UserContext) -> bool {
    match store.get_flag(key) {
        Some(f) => evaluate(&f, ctx, store),
        None => false,
    }
}

// ── Allocation profile ──────────────────────────────────────────────────────

fn allocations_per_call(mut f: impl FnMut()) -> f64 {
    const N: usize = 10_000;
    f();
    let before = ALLOCS.load(Ordering::Relaxed);
    for _ in 0..N {
        f();
    }
    (ALLOCS.load(Ordering::Relaxed) - before) as f64 / N as f64
}

fn allocation_profile() {
    let ctx = user();
    let store = store_with(1_000);
    store.upsert_flag(targeted("t10", 10));
    store.upsert_flag(rollout("r"));
    store.upsert_flag(multivariate("mv"));
    let head = chain(&store, 5);
    for (name, per) in [
        (
            "boolean",
            allocations_per_call(|| {
                black_box(eval_key(&store, "flag-500", &ctx));
            }),
        ),
        (
            "missing",
            allocations_per_call(|| {
                black_box(eval_key(&store, "absent", &ctx));
            }),
        ),
        (
            "targeting_10_rules",
            allocations_per_call(|| {
                black_box(eval_key(&store, "t10", &ctx));
            }),
        ),
        (
            "rollout",
            allocations_per_call(|| {
                black_box(eval_key(&store, "r", &ctx));
            }),
        ),
        (
            "multivariate",
            allocations_per_call(|| {
                black_box(evaluate_variant(
                    &store.get_flag("mv").unwrap(),
                    &ctx,
                    &store,
                ));
            }),
        ),
        (
            "prerequisite_chain_5",
            allocations_per_call(|| {
                black_box(eval_key(&store, &head, &ctx));
            }),
        ),
    ] {
        println!(
            "{}",
            json!({ "profile": "allocations_per_evaluation", "case": name, "allocations": per })
        );
    }
    for n in SIZES {
        let before = LIVE_BYTES.load(Ordering::Relaxed);
        let s = store_with(n);
        let after = LIVE_BYTES.load(Ordering::Relaxed);
        println!(
            "{}",
            json!({ "profile": "store_memory", "flags": n, "retained_bytes": after - before,
                    "bytes_per_flag": (after - before) as f64 / n as f64 })
        );
        drop(s);
    }
    // Replacing every flag repeatedly must not grow retained memory.
    let s = store_with(1_000);
    let base = LIVE_BYTES.load(Ordering::Relaxed);
    for _ in 0..20 {
        for i in 0..1_000 {
            s.upsert_flag(boolean(&format!("flag-{i}")));
        }
    }
    let grown = LIVE_BYTES.load(Ordering::Relaxed) as i64 - base as i64;
    println!(
        "{}",
        json!({ "profile": "replacement_growth", "flags": 1000, "rounds": 20, "retained_delta_bytes": grown })
    );
}

// ── Criterion groups ────────────────────────────────────────────────────────

fn lookup(c: &mut Criterion) {
    let ctx = user();
    let mut g = c.benchmark_group("boolean_lookup_eval");
    g.throughput(Throughput::Elements(1));
    for n in SIZES {
        let store = store_with(n);
        let key = format!("flag-{}", n / 2);
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| eval_key(&store, black_box(&key), &ctx))
        });
    }
    g.finish();

    let mut g = c.benchmark_group("missing_flag");
    g.throughput(Throughput::Elements(1));
    for n in SIZES {
        let store = store_with(n);
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| eval_key(&store, black_box("absent-flag"), &ctx))
        });
    }
    g.finish();
}

fn rules(c: &mut Criterion) {
    let ctx = user();
    let store = store_with(1_000);
    store.upsert_flag(targeted("t1", 1));
    store.upsert_flag(targeted("t10", 10));
    store.upsert_flag(rollout("rollout"));
    store.upsert_flag(multivariate("mv"));
    let mut g = c.benchmark_group("evaluation_1000_flags");
    g.throughput(Throughput::Elements(1));
    g.bench_function("targeting_1_rule", |b| {
        b.iter(|| eval_key(&store, black_box("t1"), &ctx))
    });
    g.bench_function("targeting_10_rules", |b| {
        b.iter(|| eval_key(&store, black_box("t10"), &ctx))
    });
    g.bench_function("percentage_rollout", |b| {
        b.iter(|| eval_key(&store, black_box("rollout"), &ctx))
    });
    g.bench_function("weighted_multivariate", |b| {
        b.iter(|| evaluate_variant(&store.get_flag(black_box("mv")).unwrap(), &ctx, &store))
    });
    // Depth counts flags in the chain: 2 = a flag with one prerequisite.
    for depth in [2usize, 3, 5] {
        let s = store_with(1_000);
        let head = chain(&s, depth);
        g.bench_with_input(
            BenchmarkId::new("prerequisite_chain", depth),
            &depth,
            |b, _| b.iter(|| eval_key(&s, black_box(&head), &ctx)),
        );
    }
    g.finish();
}

/// Aggregate throughput with `threads` evaluating concurrently, and with one
/// writer replacing the evaluated flags the whole time.
fn concurrency(c: &mut Criterion) {
    let threads_available = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut g = c.benchmark_group("concurrent_evaluation_1000_flags");
    g.throughput(Throughput::Elements(1));
    for threads in [1usize, 2, 4, threads_available] {
        let store = Arc::new(store_with(1_000));
        g.bench_with_input(
            BenchmarkId::new("readers", threads),
            &threads,
            |b, &threads| b.iter_custom(|iters| run_readers(&store, threads, iters, false)),
        );
    }
    for threads in [1usize, 4] {
        let store = Arc::new(store_with(1_000));
        g.bench_with_input(
            BenchmarkId::new("readers_with_writer", threads),
            &threads,
            |b, &threads| b.iter_custom(|iters| run_readers(&store, threads, iters, true)),
        );
    }
    g.finish();
}

/// Runs `iters` evaluations split across `threads`; returns wall time.
fn run_readers(store: &Arc<FlagStore>, threads: usize, iters: u64, writer: bool) -> Duration {
    let stop = AtomicBool::new(false);
    let per_thread = iters.div_ceil(threads as u64);
    std::thread::scope(|scope| {
        if writer {
            let store = Arc::clone(store);
            let stop = &stop;
            scope.spawn(move || {
                let mut i = 0usize;
                while !stop.load(Ordering::Relaxed) {
                    store.upsert_flag(boolean(&format!("flag-{}", i % 1_000)));
                    i += 1;
                }
            });
        }
        // Thread start-up (and building each thread's key list) is inside the
        // measured window; with Criterion's iteration counts it is negligible.
        let start = Instant::now();
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let store = Arc::clone(store);
                scope.spawn(move || {
                    let ctx = user();
                    let keys: Vec<String> = (0..1_000)
                        .map(|i| format!("flag-{}", (i + t * 97) % 1_000))
                        .collect();
                    for i in 0..per_thread {
                        black_box(eval_key(&store, &keys[i as usize % 1_000], &ctx));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let elapsed = start.elapsed();
        stop.store(true, Ordering::Relaxed);
        elapsed
    })
}

/// What an SDK does per change event: parse the JSON payload and replace the
/// flag in the store.
fn apply_update(c: &mut Criterion) {
    let store = store_with(1_000);
    let event = json!({ "type": "UPSERT", "env_id": "e", "revision": 42,
        "flag": serde_json::to_value(targeted("flag-500", 3)).unwrap() })
    .to_string();
    c.bench_function("sdk_apply_upsert_event", |b| {
        b.iter(|| {
            let v: serde_json::Value = serde_json::from_str(black_box(&event)).unwrap();
            let f: Flag = serde_json::from_value(v["flag"].clone()).unwrap();
            store.upsert_flag(f);
        })
    });
}

criterion_group!(benches, lookup, rules, concurrency, apply_update);

fn main() {
    allocation_profile();
    benches();
    Criterion::default().configure_from_args().final_summary();
}
