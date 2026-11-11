#!/usr/bin/env python3
"""Turn one Criterion run of bench/benches/evaluation.rs into structured JSON.

    extract_criterion.py <criterion-dir> <bench-stdout.txt> > evaluation-rust-runN.json

<criterion-dir> is the `criterion` directory of the run (estimates.json and
sample.json under each benchmark's `new/`). <bench-stdout.txt> is the bench's
stdout, whose leading JSON lines are the counting allocator's profile.

Reported per benchmark:
- estimate_ns: the figure Criterion prints (the slope estimate in linear
  sampling mode), with its 95 % bootstrap confidence interval; ops_per_s is
  1e9 / estimate_ns (one thread).
- Criterion's mean and median with their 95 % confidence intervals, and the
  standard deviation.
- The spread of Criterion's samples: each of the ~100 samples is the mean
  time per iteration over many iterations, so p95/p99 here describe how much
  those sample means varied, not single-evaluation latency.
"""

import json
import os
import sys


def pct(sorted_values, p):
    return sorted_values[round((len(sorted_values) - 1) * p)]


def r3(v):
    return round(v, 3)


def load_benchmark(new_dir):
    with open(os.path.join(new_dir, "benchmark.json")) as f:
        meta = json.load(f)
    with open(os.path.join(new_dir, "estimates.json")) as f:
        est = json.load(f)
    with open(os.path.join(new_dir, "sample.json")) as f:
        sample = json.load(f)
    per_iter = sorted(t / i for t, i in zip(sample["times"], sample["iters"]))
    mean = est["mean"]
    median = est["median"]
    # The figure Criterion prints: the slope of a linear fit of sample time
    # against iteration count (linear sampling mode), else the mean.
    headline = est.get("slope") or mean
    ci = headline["confidence_interval"]
    return meta["full_id"], {
        "estimate": "slope" if est.get("slope") else "mean",
        "estimate_ns": r3(headline["point_estimate"]),
        "estimate_ci95_ns": [r3(ci["lower_bound"]), r3(ci["upper_bound"])],
        "ops_per_s": round(1e9 / headline["point_estimate"]),
        "mean_ns": r3(mean["point_estimate"]),
        "mean_ci95_ns": [r3(mean["confidence_interval"]["lower_bound"]),
                         r3(mean["confidence_interval"]["upper_bound"])],
        "median_ns": r3(median["point_estimate"]),
        "median_ci95_ns": [r3(median["confidence_interval"]["lower_bound"]),
                           r3(median["confidence_interval"]["upper_bound"])],
        "std_dev_ns": r3(est["std_dev"]["point_estimate"]),
        "samples": len(per_iter),
        "iterations": int(sum(sample["iters"])),
        "sample_mean_p50_ns": r3(pct(per_iter, 0.50)),
        "sample_mean_p95_ns": r3(pct(per_iter, 0.95)),
        "sample_mean_p99_ns": r3(pct(per_iter, 0.99)),
        "sample_mean_max_ns": r3(per_iter[-1]),
    }


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    root, stdout_path = sys.argv[1], sys.argv[2]
    benchmarks = {}
    for dirpath, dirnames, filenames in os.walk(root):
        if os.path.basename(dirpath) == "new" and "benchmark.json" in filenames:
            name, data = load_benchmark(dirpath)
            benchmarks[name] = data
    if not benchmarks:
        sys.exit(f"no Criterion results under {root}")

    allocations, store_memory, growth = {}, [], None
    with open(stdout_path, encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.strip()
            if not line.startswith("{"):
                continue
            try:
                rec = json.loads(line)
            except ValueError:
                continue
            kind = rec.get("profile")
            if kind == "allocations_per_evaluation":
                allocations[rec["case"]] = rec["allocations"]
            elif kind == "store_memory":
                store_memory.append({"flags": rec["flags"], "bytes_per_flag": r3(rec["bytes_per_flag"])})
            elif kind == "replacement_growth":
                growth = rec["retained_delta_bytes"]

    json.dump({
        "generated_by": "bench/public/scripts/extract_criterion.py",
        "harness": "bench/benches/evaluation.rs (Criterion, release profile)",
        "allocations_per_evaluation": allocations,
        "store_memory": store_memory,
        "replacement_growth_bytes": growth,
        "benchmarks": dict(sorted(benchmarks.items())),
    }, sys.stdout, indent=2)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
