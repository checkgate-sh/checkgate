#!/usr/bin/env python3
"""Aggregate the raw benchmark runs into summary.json.

    summarize.py docs/performance/results/v0.1.31 > .../summary.json

The version is the directory's name (v0.1.31). A `baseline-v<version>/`
directory inside it (written by compare.sh: the same harness, host and
limits, repetitions alternated) is aggregated the same way and compared.

Aggregation, fixed in advance (never "the best run"):
- Evaluation: per case, the median of the per-run estimates, and the range.
- Throughput: per concurrency level, the median across runs of successful
  req/s and of each latency percentile, the range of req/s, and the worst
  error rate. CPU mean is the median of run means; CPU and RSS peaks are the
  maximum over all runs.
- Propagation: percentiles pooled over every (update, client) sample of every
  run (from 0.1 ms histograms), plus each run's own p99 so spread is visible.
  Missed updates and reconnects are summed over runs.

Headline values (the README graphic), also fixed in advance:
- evaluation: Node SDK isEnabled(), 50 % percentage rollout, 1 000-flag
  store: median of batch means, with their p95 and p99 (a batch is 20 000
  calls: single calls are too short to time), and the Rust core alone.
- throughput: the concurrency level with the highest median req/s among
  levels with zero errors in every run.
- propagation: p99 at the largest client count that completed in every run.
- efficiency: server CPU per snapshot request and peak RSS at the
  throughput headline's concurrency.
Each headline also carries the baseline's value for the same metric (for
throughput and efficiency: at the same concurrency level), when a baseline
was measured.
"""

import glob
import json
import os
import re
import statistics
import sys

RUST_CASES = {
    "boolean": ("boolean_lookup_eval/1000", "boolean"),
    "targeting_1_rule": ("evaluation_1000_flags/targeting_1_rule", None),
    "targeting_10_rules": ("evaluation_1000_flags/targeting_10_rules", "targeting_10_rules"),
    "percentage_rollout": ("evaluation_1000_flags/percentage_rollout", "rollout"),
    "weighted_multivariate": ("evaluation_1000_flags/weighted_multivariate", "multivariate"),
    "prerequisite_chain_5": ("evaluation_1000_flags/prerequisite_chain/5", "prerequisite_chain_5"),
}


def load(path):
    with open(path) as f:
        return json.load(f)


def runs(raw, pattern):
    files = sorted(glob.glob(os.path.join(raw, pattern)),
                   key=lambda p: int(re.search(r"run(\d+)", p).group(1)))
    return [(p, load(p)) for p in files]


def med(values):
    values = [v for v in values if v is not None]
    return round(statistics.median(values), 3) if values else None


def r3(v):
    return None if v is None else round(v, 3)


def evaluation(raw):
    rust_runs = runs(raw, "evaluation-rust-run*.json")
    node_runs = runs(raw, "evaluation-node-run*.json")
    rust = {}
    for name, (bench_id, alloc_key) in RUST_CASES.items():
        ests = [d["benchmarks"][bench_id]["estimate_ns"] for _, d in rust_runs if bench_id in d["benchmarks"]]
        if not ests:
            continue
        allocs = {d["allocations_per_evaluation"].get(alloc_key) for _, d in rust_runs} if alloc_key else {None}
        m = statistics.median(ests)
        rust[name] = {
            "criterion_id": bench_id,
            "runs_ns": ests,
            "median_ns": r3(m),
            "range_ns": [min(ests), max(ests)],
            "ops_per_s": round(1e9 / m),
            "allocations_per_evaluation": allocs.pop() if len(allocs) == 1 else sorted(allocs),
        }
    node = {}
    for _, d in node_runs[:1]:
        for name, case in d["cases"].items():
            meds = [n["cases"][name]["median_ns"] for _, n in node_runs]
            p95s = [n["cases"][name].get("p95_ns") for _, n in node_runs]
            p99s = [n["cases"][name]["p99_ns"] for _, n in node_runs]
            m = statistics.median(meds)
            node[name] = {
                "api": case["api"],
                "runs_median_ns": meds,
                "median_ns": r3(m),
                "range_ns": [min(meds), max(meds)],
                "batch_mean_p95_ns": med(p95s),
                "batch_mean_p99_ns": med(p99s),
                "ops_per_s": round(1e9 / m),
            }
    node_meta = node_runs[0][1] if node_runs else {}
    return {
        "rust_core": {
            "harness": "bench/benches/evaluation.rs (Criterion); estimate = Criterion's printed slope estimate",
            "runs": len(rust_runs),
            "store_flags": 1000,
            "cases": rust,
        },
        "node_sdk": {
            "harness": "bench/public/sdk/eval.mjs; per run, median of 100 batch means of 20 000 calls",
            "runs": len(node_runs),
            "node": node_meta.get("node"),
            "store_flags": node_meta.get("store_flags"),
            "checks": node_meta.get("checks"),
            "cases": node,
        },
    }


def containers(raw, prefix):
    out = {}
    for _, d in runs(raw, f"{prefix}-run*.containers.json"):
        for svc, c in d["containers"].items():
            o = out.setdefault(svc, {"cpu_pct_max": 0, "cpu_pct_p90_max": 0, "mem_mib_max": 0})
            o["cpu_pct_max"] = max(o["cpu_pct_max"], c["cpu_pct_max"])
            o["cpu_pct_p90_max"] = max(o["cpu_pct_p90_max"], c["cpu_pct_p90"])
            o["mem_mib_max"] = max(o["mem_mib_max"], c["mem_mib_max"])
    return out


def throughput(raw, env):
    rs = [d for p, d in runs(raw, "throughput-run*.json") if not p.endswith(".containers.json")]
    if not rs:
        return None
    cpus = float(env["services"]["server"]["cpus"])
    levels = []
    for c in [lv["concurrency"] for lv in rs[0]["levels"]]:
        lv = [next(x for x in d["levels"] if x["concurrency"] == c) for d in rs]
        rps = [x["successful_rps"] for x in lv]
        rps_m = statistics.median(rps)
        cpu_mean = med([x["server"]["cpu_cores_mean"] for x in lv])
        levels.append({
            "concurrency": c,
            "runs": len(lv),
            "runs_rps": rps,
            "rps_median": r3(rps_m),
            "rps_range": [min(rps), max(rps)],
            "latency_ms": {k: med([x["latency"].get(f"{k}_ms") for x in lv]) for k in ("p50", "p95", "p99", "max")},
            "errors_total": sum(x["failed_requests"] for x in lv),
            "error_rate_max": max(x["error_rate"] for x in lv),
            "error_kinds": sorted({k for x in lv for k in x["errors"]}),
            "server_cpu_cores_mean": cpu_mean,
            "server_cpu_cores_peak": max(x["server"]["cpu_cores_peak_1s"] for x in lv),
            "server_rss_mib_median": med([x["server"]["rss_mib_median"] for x in lv]),
            "server_rss_mib_peak": max(x["server"]["rss_mib_peak_1s"] for x in lv),
            "generator_cpu_cores_mean": med([x["generator"]["cpu_cores_mean"] for x in lv]),
            "generator_cpu_cores_peak": max(x["generator"]["cpu_cores_peak_1s"] for x in lv),
            "rps_per_vcpu_limit": r3(rps_m / cpus),
            "requests_per_server_cpu_second": r3(rps_m / cpu_mean) if cpu_mean else None,
        })
    return {
        "endpoint": rs[0]["endpoint"],
        "flags_in_snapshot": rs[0]["flags_in_snapshot"],
        "response_bytes": rs[0]["response_bytes"],
        "server_cpus": cpus,
        "server_memory_mib": env["services"]["server"]["memory_mib"],
        "warmup_s": rs[0]["levels"][0]["warmup_s"],
        "window_s": rs[0]["levels"][0]["window_s"],
        "runs": len(rs),
        "definitions": {
            "rps_median": "successful requests per second in the measurement window, median of runs",
            "rps_per_vcpu_limit": "rps_median divided by the server's CPU quota (measured at that quota, not extrapolated)",
            "requests_per_server_cpu_second": "rps_median divided by the server CPU cores actually used (mean)",
        },
        "levels": levels,
        "containers": containers(raw, "throughput"),
    }


def pooled(hists, maxes):
    merged = {}
    for h in hists:
        for k, v in h.items():
            merged[int(k)] = merged.get(int(k), 0) + v
    total = sum(merged.values())
    if not total:
        return None
    out = {}
    for name, p in (("p50", 0.5), ("p95", 0.95), ("p99", 0.99), ("p999", 0.999)):
        rank = round((total - 1) * p)
        seen = 0
        for b in sorted(merged):
            seen += merged[b]
            if seen > rank:
                out[f"{name}_ms"] = b / 10
                break
    out["max_ms"] = max(maxes)
    out["samples"] = total
    return out


def propagation(raw):
    rs = [d for p, d in runs(raw, "propagation-run*.json") if not p.endswith(".containers.json")]
    if not rs:
        return None
    profiles = []
    for n in [p["clients"] for p in rs[0]["profiles"]]:
        ps = [next(x for x in d["profiles"] if x["clients"] == n) for d in rs]
        prop = [p["propagation"] for p in ps]
        expected = sum(x["expected_observations"] for x in prop)
        missed = sum(x["missed"] for x in prop)
        within = sum(x["within_deadline_pct"] * x["expected_observations"] / 100 for x in prop)
        profiles.append({
            "clients": n,
            "runs": len(ps),
            "updates": sum(x["updates"] for x in prop),
            "expected_observations": expected,
            "missed": missed,
            "connect_failures": sum(len(p["connect"]["failures"]) for p in ps),
            "wrong_initial_value": sum(p["connect"]["clients_with_wrong_initial_value"] for p in ps),
            "reconnects": sum(p["stream_health"]["reconnects"] for p in ps),
            "clients_polling_at_end": sum(p["stream_health"]["clients_polling_at_end"] for p in ps),
            "deadline_ms": prop[0]["deadline_ms"],
            "within_deadline_pct": round(100 * within / expected, 3) if expected else None,
            "visible_from_request_ms": pooled([x["visible_from_request_histogram_0_1ms"] for x in prop],
                                              [x["visible_from_request"]["max_ms"] for x in prop]),
            "runs_p99_ms": [x["visible_from_request"]["p99_ms"] for x in prop],
            # The write path's own p99 per run: when it moves with the delivery p99,
            # the whole stack was slow (host contention), not delivery.
            "runs_patch_response_p99_ms": [x["patch_response"]["p99_ms"] for x in prop],
            "visible_after_response_p99_ms": med([x["visible_after_response"]["p99_ms"] for x in prop]),
            "patch_response_p50_ms": med([x["patch_response"]["p50_ms"] for x in prop]),
            "observed_before_response_pct": round(
                100 * sum(x["observed_before_patch_response"] for x in prop) / max(1, expected - missed), 1),
            "time_to_ready_p99_ms": med([p["connect"]["time_to_ready"].get("p99_ms") for p in ps]),
            "server_rss_mib_held": med([p["held_idle"]["server_rss_mib"] for p in ps]),
            "server_cpu_cores_during_propagation": med([p["resources"]["server_cpu_cores_during_propagation"] for p in ps]),
            "sdk_workers_cpu_cores_max": max(p["resources"]["sdk_workers_cpu_cores"] for p in ps),
        })
    return {
        "sdk": rs[0]["sdk"],
        "primary_metric": rs[0]["primary_metric"],
        "runs": len(rs),
        "gap_ms": rs[0]["gap_ms"],
        "profiles": profiles,
        "containers": containers(raw, "propagation"),
    }


def run_log(raw):
    entries = []
    path = os.path.join(raw, "runs.jsonl")
    if os.path.exists(path):
        with open(path) as f:
            entries = [json.loads(line) for line in f if line.strip()]
    failed_files = sorted(os.path.basename(p) for p in glob.glob(os.path.join(raw, "failed", "*")))
    # Complete runs replaced after a harness fix; each directory's README says why.
    superseded = sorted(os.path.basename(p) for p in glob.glob(os.path.join(raw, "superseded-*")))
    return entries, failed_files, superseded


def fmt_int(v):
    return f"{int(round(v)):,}"


def fmt_ns(v):
    """Nanoseconds at the precision the runs support (two significant figures above 1 µs)."""
    return f"{v / 1000:.1f} µs" if v >= 1000 else f"{round(v)} ns"


def best_level(tp):
    clean = [lv for lv in tp["levels"] if lv["errors_total"] == 0] if tp else []
    return max(clean, key=lambda lv: lv["rps_median"]) if clean else None


def top_profile(pr):
    complete = [p for p in pr["profiles"] if p["runs"] == pr["runs"] and p["visible_from_request_ms"]] if pr else []
    return max(complete, key=lambda p: p["clients"]) if complete else None


def headline(ev, tp, pr, env):
    h = {}
    node = ev["node_sdk"]["cases"].get("percentage_rollout")
    rust = ev["rust_core"]["cases"].get("percentage_rollout")
    if node:
        h["evaluation"] = {
            "value": node["median_ns"], "unit": "ns",
            "scenario": "Node SDK isEnabled(): 50% rollout, 1,000-flag store, no network call",
            "context": (f"batch means p95 {fmt_ns(node['batch_mean_p95_ns'])} · p99 {fmt_ns(node['batch_mean_p99_ns'])}"
                        + (f" · Rust core alone {fmt_ns(rust['median_ns'])}" if rust else "")),
            "p95": node["batch_mean_p95_ns"], "p99": node["batch_mean_p99_ns"],
            "source": "evaluation.node_sdk.cases.percentage_rollout.median_ns",
        }
    best = best_level(tp)
    if best:
        h["throughput"] = {
            "value": best["rps_median"], "unit": "req/s",
            "scenario": f"SDK snapshot reads, {fmt_int(best['concurrency'])} concurrent clients, "
                        f"p99 {round(best['latency_ms']['p99'])} ms",
            "context": f"one server limited to {tp['server_cpus']:g}\u00a0vCPU · {tp['server_memory_mib']}\u00a0MiB",
            "source": f"throughput.levels[concurrency={best['concurrency']}].rps_median",
        }
        if best["requests_per_server_cpu_second"]:
            h["efficiency"] = {
                "value": r3(1e6 / best["requests_per_server_cpu_second"]), "unit": "µs",
                "scenario": "server CPU per snapshot request (101 flags, 15 KB)",
                "context": f"peak RSS {round(best['server_rss_mib_peak'])} MiB under that load",
                "rss_peak_mib": best["server_rss_mib_peak"],
                "source": f"1e6 / throughput.levels[concurrency={best['concurrency']}].requests_per_server_cpu_second",
            }
    top = top_profile(pr)
    if top:
        h["propagation"] = {
            "value": top["visible_from_request_ms"]["p99_ms"], "unit": "ms",
            "scenario": f"p99 update → visible in SDK, {fmt_int(top['clients'])} connected SDK clients",
            "context": f"p50 {top['visible_from_request_ms']['p50_ms']:g} ms · {top['missed']} of "
                       f"{fmt_int(top['expected_observations'])} missed · pooled over {top['runs']} runs; "
                       f"median run p99 {round(med(top['runs_p99_ms']))} ms",
            "source": f"propagation.profiles[clients={top['clients']}].visible_from_request_ms.p99_ms",
            "runs_p99_median_ms": med(top["runs_p99_ms"]),
        }
    return h


def ratio(base, cand):
    return round(base / cand, 2) if base and cand else None


def comparison(out_dir, cand_ev, cand_tp, cand_pr, cand_head):
    """The baseline (same harness, host, limits; alternated runs) side by side."""
    dirs = sorted(glob.glob(os.path.join(out_dir, "baseline-v*")))
    if not dirs:
        return None, None
    bdir = dirs[0]
    braw = os.path.join(bdir, "raw")
    benv = load(os.path.join(bdir, "environment.json"))
    bev, btp, bpr = evaluation(braw), throughput(braw, benv), propagation(braw)
    entries, failed, _ = run_log(braw)
    by_phase = {}
    for e in entries:
        if e.get("superseded"):
            continue
        b = by_phase.setdefault(e["phase"], {"ok": 0, "failed": 0})
        b["ok" if e["status"] == "ok" else "failed"] += 1
    bhead = headline(bev, btp, bpr, benv)
    for key, h in cand_head.items():
        if key in bhead and bhead[key]["source"] == h["source"]:
            h["baseline_value"] = bhead[key]["value"]
            if "runs_p99_median_ms" in h:
                h["baseline_runs_p99_median_ms"] = bhead[key].get("runs_p99_median_ms")
    # Throughput and efficiency headlines pick a concurrency level; compare
    # with the baseline at that same level, not at the baseline's own best.
    best = best_level(cand_tp)
    same = next((lv for lv in (btp or {}).get("levels", []) if best and lv["concurrency"] == best["concurrency"]), None)
    if same:
        if "throughput" in cand_head:
            cand_head["throughput"]["baseline_value"] = same["rps_median"]
        if "efficiency" in cand_head and same["requests_per_server_cpu_second"]:
            cand_head["efficiency"]["baseline_value"] = r3(1e6 / same["requests_per_server_cpu_second"])
    node = {}
    for case, c in cand_ev["node_sdk"]["cases"].items():
        b = bev["node_sdk"]["cases"].get(case)
        if b:
            node[case] = {"baseline_ns": b["median_ns"], "candidate_ns": c["median_ns"],
                          "speedup": ratio(b["median_ns"], c["median_ns"])}
    rust = {}
    for case, c in cand_ev["rust_core"]["cases"].items():
        b = bev["rust_core"]["cases"].get(case)
        if b:
            rust[case] = {"baseline_ns": b["median_ns"], "candidate_ns": c["median_ns"],
                          "speedup": ratio(b["median_ns"], c["median_ns"])}
    tput = []
    for lv in (cand_tp or {}).get("levels", []):
        b = next((x for x in (btp or {}).get("levels", []) if x["concurrency"] == lv["concurrency"]), None)
        if b:
            cpu_b = r3(1e6 / b["requests_per_server_cpu_second"]) if b["requests_per_server_cpu_second"] else None
            cpu_c = r3(1e6 / lv["requests_per_server_cpu_second"]) if lv["requests_per_server_cpu_second"] else None
            tput.append({"concurrency": lv["concurrency"],
                         "baseline_rps": b["rps_median"], "candidate_rps": lv["rps_median"],
                         "rps_ratio": ratio(lv["rps_median"], b["rps_median"]),
                         "baseline_cpu_us_per_request": cpu_b, "candidate_cpu_us_per_request": cpu_c,
                         "baseline_errors": b["errors_total"], "candidate_errors": lv["errors_total"],
                         "baseline_rss_mib_peak": b["server_rss_mib_peak"],
                         "candidate_rss_mib_peak": lv["server_rss_mib_peak"]})
    prop = []
    for p in (cand_pr or {}).get("profiles", []):
        b = next((x for x in (bpr or {}).get("profiles", []) if x["clients"] == p["clients"]), None)
        if b and b["visible_from_request_ms"] and p["visible_from_request_ms"]:
            prop.append({"clients": p["clients"],
                         "baseline_p50_ms": b["visible_from_request_ms"]["p50_ms"],
                         "candidate_p50_ms": p["visible_from_request_ms"]["p50_ms"],
                         "baseline_p99_ms": b["visible_from_request_ms"]["p99_ms"],
                         "candidate_p99_ms": p["visible_from_request_ms"]["p99_ms"],
                         "baseline_runs_p99_ms": b["runs_p99_ms"], "candidate_runs_p99_ms": p["runs_p99_ms"],
                         "baseline_runs_patch_response_p99_ms": b["runs_patch_response_p99_ms"],
                         "candidate_runs_patch_response_p99_ms": p["runs_patch_response_p99_ms"],
                         "baseline_missed": b["missed"], "candidate_missed": p["missed"]})
    baseline = {
        "version": os.path.basename(bdir).replace("baseline-v", ""),
        "commit": benv["git"]["commit"],
        "measured_source_matches_commit": benv["git"]["measured_source_matches_commit"],
        "runs": by_phase,
        "failed_run_files": failed,
        "method": "bench/public/compare.sh: this harness run against the baseline's source in a worktree, "
                  "same host and limits, repetitions alternated between the two versions",
    }
    return baseline, {"node_sdk": node, "rust_core": rust, "throughput": tput, "propagation": prop}


def main():
    out_dir = sys.argv[1]
    raw = os.path.join(out_dir, "raw")
    env = load(os.path.join(out_dir, "environment.json"))
    ev = evaluation(raw)
    tp = throughput(raw, env)
    pr = propagation(raw)
    entries, failed_files, superseded = run_log(raw)
    head = headline(ev, tp, pr, env)
    baseline, compare = comparison(out_dir, ev, tp, pr, head)
    by_phase = {}
    for e in entries:
        if e.get("superseded"):
            continue
        b = by_phase.setdefault(e["phase"], {"ok": 0, "failed": 0})
        b["ok" if e["status"] == "ok" else "failed"] += 1
    dates = sorted(e["ended"][:10] for e in entries)
    h = env["host"]
    summary = {
        "schema": "checkgate-benchmarks/v1",
        "version": os.path.basename(os.path.normpath(out_dir)).lstrip("v"),
        "commit": env["git"]["commit"],
        "measured_source_matches_commit": env["git"]["measured_source_matches_commit"],
        "date": dates[-1] if dates else None,
        "environment": {
            "host": f"{h['cpu']}, {h.get('physical_cores', '?')} cores / {h['logical_cpus']} threads, "
                    f"{h['memory_gib']:g} GiB, {h['os']}",
            "short_host": re.sub(r"\(R\)|\(TM\)|CPU |@ .*", "", h["cpu"] or "").strip(),
            "docker": f"{env['docker']['operating_system']} {env['docker']['server_version']}, "
                      f"VM {env['docker']['vm_cpus']} vCPU / {env['docker']['vm_memory_gib']:g} GiB",
            "server_limits": f"{env['services']['server']['cpus']} vCPU, {env['services']['server']['memory_mib']} MiB",
            "postgres": env["services_versions"]["postgres"],
            "redis": env["services_versions"]["redis"],
            "rustc": env["toolchains"]["rustc"],
        },
        "runs": by_phase,
        # The fewest successful runs of any phase, so a claim of "N runs" holds for all.
        "repetitions": min((b["ok"] for b in by_phase.values()), default=0),
        "failed_run_files": failed_files,
        "superseded_run_sets": superseded,
        "aggregation": __doc__.split("Aggregation, fixed in advance (never \"the best run\"):")[1].split("Headline")[0].strip(),
        "evaluation": ev,
        "throughput": tp,
        "propagation": pr,
        "headline": head,
    }
    if baseline:
        summary["baseline"] = baseline
        summary["comparison"] = compare
    json.dump(summary, sys.stdout, indent=2, ensure_ascii=False)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
