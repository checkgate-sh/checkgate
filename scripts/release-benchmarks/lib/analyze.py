#!/usr/bin/env python3
"""Regression analysis for scripts/release-benchmarks/v0.2.0-linux.sh.

    analyze.py <output-dir> [--threshold 0.10] [--min-runs 3] [--seed 20261122]

Reads <output-dir>/raw/runs.jsonl (one record per benchmark run, written by
the driver) and the result files it points to, and writes
<output-dir>/summary.json and <output-dir>/summary.md. Deterministic: the same
inputs and seed give the same verdicts.

Method
------
Each run yields one value per metric (for example the median ns of a core
case, or the throughput at one concurrency level). Per metric:

* Only *valid* runs count. A run is invalid when its harness failed, the
  server exited during it, the host 1-minute load before it exceeded the
  limit the driver recorded, host CPU steal exceeded 5 %, or swap use grew
  by more than 64 MiB during it. The reasons are written next to each run.
  A server that exits mid-run additionally fails the stability gate:
  excluding the run never hides a crash.
* ratio = median(candidate) / median(baseline). Its 95 % confidence interval
  comes from a percentile bootstrap that resamples runs within each side
  (10,000 resamples, fixed seed).
* Verdict against the regression threshold t (default 10 %):
    lower is better  (latency, ns, CPU, RSS): PASS if CI high <= 1 + t,
                                               FAIL if CI low  >  1 + t
    higher is better (throughput, req/CPU-s):  PASS if CI low  >= 1 - t,
                                               FAIL if CI high <  1 - t
  and INCONCLUSIVE otherwise, or when either side has fewer than --min-runs
  valid runs, or either side's run-to-run coefficient of variation exceeds t
  (too noisy to resolve a t-sized change).
* Correctness counters (missed / duplicated / out-of-order updates,
  unexpected reconnects and disconnects, HTTP errors, server exits) are not
  compared: the candidate must be 0 in every valid run, else FAIL (the
  baseline's count is shown beside it).

The overall verdict is FAIL if any gating metric fails, else INCONCLUSIVE if
any gating metric is inconclusive or a suite has no valid runs, else PASS.
"""

import argparse
import json
import os
import random
import statistics as st
import sys

LOWER, HIGHER, ZERO = "lower_is_better", "higher_is_better", "must_be_zero"


def load(path):
    with open(path) as f:
        return json.load(f)


def last_summary(path):
    """The {"summary": ...} line a sampler file ends with."""
    if not path or not os.path.exists(path):
        return None
    summary = None
    with open(path) as f:
        for line in f:
            row = json.loads(line)
            if "summary" in row:
                summary = row["summary"]
    return summary


# ── Metric extraction, per suite ────────────────────────────────────────────
# Each returns {metric: (value, direction, gating)}.


def core_metrics(run, out):
    doc = load(os.path.join(out, run["result"]))
    return {
        f"core.{case}.median_ns": (v["median_ns"], LOWER, True)
        for case, v in doc["cases"].items()
    }


def node_metrics(run, out):
    doc = load(os.path.join(out, run["result"]))
    m = {}
    for case, v in doc["cases"].items():
        m[f"node.{case}.median_ns"] = (v["median_ns"], LOWER, True)
        m[f"node.{case}.p95_ns"] = (v["p95_ns"], LOWER, False)
        m[f"node.{case}.p99_ns"] = (v["p99_ns"], LOWER, False)
    return m


def snapshot_metrics(run, out):
    doc = load(os.path.join(out, run["result"]))
    m = {}
    errors = 0
    for lvl in doc["levels"]:
        c = lvl["concurrency"]
        p = f"snapshot.c{c}"
        m[f"{p}.throughput_rps"] = (lvl["successful_rps"], HIGHER, True)
        m[f"{p}.requests_per_cpu_second"] = (
            lvl["server"]["requests_per_cpu_second"],
            HIGHER,
            True,
        )
        m[f"{p}.cpu_us_per_request"] = (
            1e6 / lvl["server"]["requests_per_cpu_second"]
            if lvl["server"]["requests_per_cpu_second"]
            else None,
            LOWER,
            False,
        )
        m[f"{p}.latency_p50_ms"] = (lvl["latency"].get("p50_ms"), LOWER, False)
        m[f"{p}.latency_p99_ms"] = (lvl["latency"].get("p99_ms"), LOWER, False)
        m[f"{p}.error_rate"] = (lvl["error_rate"], ZERO, True)
        errors += lvl["failed_requests"]
    m["snapshot.failed_requests"] = (errors, ZERO, True)
    m["snapshot.peak_rss_mib"] = (doc["server_rss_mib_high_water_mark"], LOWER, True)
    s = last_summary(run.get("sampler") and os.path.join(out, run["sampler"]))
    if s:
        m["snapshot.server_cpu_cores_mean_whole_run"] = (s["server_cpu_cores_mean"], LOWER, False)
        m["snapshot.peak_threads"] = (s["peak"]["threads"], LOWER, False)
    return m


def sse_metrics(run, out):
    doc = load(os.path.join(out, run["result"]))
    m = {}
    for prof in doc["profiles"]:
        n = prof["clients"]
        p = f"sse.n{n}"
        vis = prof["propagation"]["visible_from_request"]
        for q in ("p50", "p95", "p99"):
            m[f"{p}.propagation_{q}_ms"] = (vis.get(f"{q}_ms"), LOWER, True)
        m[f"{p}.missed_updates"] = (prof["propagation"]["missed"], ZERO, True)
        m[f"{p}.unexpected_reconnects"] = (prof["stream_health"]["reconnects"], ZERO, True)
        failures = prof["connect"]["failures"]  # a list of messages
        m[f"{p}.connect_failures"] = (len(failures) if isinstance(failures, list) else failures, ZERO, True)
        m[f"{p}.wrong_initial_value"] = (
            prof["connect"]["clients_with_wrong_initial_value"],
            ZERO,
            True,
        )
        res = prof["resources"]
        m[f"{p}.server_cpu_cores_during_propagation"] = (
            res["server_cpu_cores_during_propagation"],
            LOWER,
            True,
        )
        m[f"{p}.server_rss_mib_after_propagation"] = (
            res["server_rss_mib_after_propagation"],
            LOWER,
            False,
        )
        m[f"{p}.held_idle_server_rss_mib"] = (prof["held_idle"]["server_rss_mib"], LOWER, False)
    if run.get("audit") and os.path.exists(os.path.join(out, run["audit"])):
        a = load(os.path.join(out, run["audit"]))
        m["sse.audit.duplicated_updates"] = (a["duplicated"], ZERO, True)
        m["sse.audit.out_of_order_updates"] = (a["out_of_order"], ZERO, True)
        m["sse.audit.stream_disconnects"] = (a["disconnects"], ZERO, True)
        m["sse.audit.http_errors"] = (a["http_errors"], ZERO, True)
        m["sse.audit.updates_checked"] = (a["updates_after_ready"], None, False)
    s = last_summary(run.get("sampler") and os.path.join(out, run["sampler"]))
    if s:
        m["sse.peak_rss_mib"] = (s["peak"]["hwm_mib"], LOWER, True)
        m["sse.peak_connections"] = (s["peak"]["connections"], None, False)
        m["sse.peak_fds"] = (s["peak"]["fds"], None, False)
        m["sse.peak_threads"] = (s["peak"]["threads"], LOWER, False)
    return m


EXTRACT = {"core": core_metrics, "node": node_metrics, "snapshot": snapshot_metrics, "sse": sse_metrics}


# ── Statistics ──────────────────────────────────────────────────────────────


def cv(values):
    if len(values) < 2:
        return 0.0
    mean = st.fmean(values)
    return st.pstdev(values) / mean if mean else 0.0


def bootstrap_ratio(base, cand, rng, iters=10_000):
    ratios = []
    for _ in range(iters):
        b = st.median(rng.choices(base, k=len(base)))
        c = st.median(rng.choices(cand, k=len(cand)))
        if b:
            ratios.append(c / b)
    ratios.sort()
    lo = ratios[int(0.025 * (len(ratios) - 1))]
    hi = ratios[int(0.975 * (len(ratios) - 1))]
    return lo, hi


def verdict_perf(direction, base, cand, t, min_runs, rng):
    row = {
        "baseline": {"runs": len(base), "median": st.median(base) if base else None, "cv": round(cv(base), 4)},
        "candidate": {"runs": len(cand), "median": st.median(cand) if cand else None, "cv": round(cv(cand), 4)},
    }
    if len(base) < min_runs or len(cand) < min_runs:
        row.update(verdict="INCONCLUSIVE", why=f"fewer than {min_runs} valid runs per side")
        return row
    b, c = st.median(base), st.median(cand)
    if not b:
        row.update(verdict="INCONCLUSIVE", why="baseline median is 0")
        return row
    ratio = c / b
    lo, hi = bootstrap_ratio(base, cand, rng)
    row.update(ratio=round(ratio, 4), ci95=[round(lo, 4), round(hi, 4)])
    noisy = max(cv(base), cv(cand)) > t
    if direction == LOWER:
        passed, failed = hi <= 1 + t, lo > 1 + t
    else:
        passed, failed = lo >= 1 - t, hi < 1 - t
    if failed:
        row.update(verdict="FAIL", why=f"regression beyond {t:.0%} at 95 % confidence")
    elif noisy:
        row.update(verdict="INCONCLUSIVE", why=f"run-to-run variation above {t:.0%}")
    elif passed:
        worse = lo > 1 if direction == LOWER else hi < 1
        why = f"no regression beyond {t:.0%} at 95 % confidence"
        if worse:
            # Shown, not hidden: a real change in the unfavourable direction
            # that stays inside the threshold.
            why += f"; candidate measurably worse by {abs(ratio - 1):.1%}"
        row.update(verdict="PASS", why=why, measurably_worse=worse)
    else:
        row.update(verdict="INCONCLUSIVE", why=f"95 % CI includes a regression beyond {t:.0%}")
    return row


def verdict_zero(base, cand):
    row = {
        "baseline": {"runs": len(base), "total": sum(base) if base else None},
        "candidate": {"runs": len(cand), "total": sum(cand) if cand else None},
    }
    if not cand:
        row.update(verdict="INCONCLUSIVE", why="no valid candidate run")
    elif sum(cand) > 0:
        also = " (baseline also non-zero)" if base and sum(base) > 0 else ""
        row.update(verdict="FAIL", why=f"candidate total {sum(cand):g}, must be 0{also}")
    else:
        row.update(verdict="PASS", why="0 in every valid candidate run")
    return row


# ── Report ──────────────────────────────────────────────────────────────────


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--threshold", type=float, default=0.10)
    ap.add_argument("--min-runs", type=int, default=3)
    ap.add_argument("--seed", type=int, default=20261122)
    a = ap.parse_args()
    out = a.out
    rng = random.Random(a.seed)

    runs = []
    with open(os.path.join(out, "raw", "runs.jsonl")) as f:
        for line in f:
            if line.strip():
                runs.append(json.loads(line))
    meta = load(os.path.join(out, "environment.json")) if os.path.exists(os.path.join(out, "environment.json")) else {}

    values = {}  # metric -> side -> [values]
    meta_metric = {}
    stability = {"baseline": 0, "candidate": 0}
    suites = {}
    for run in runs:
        s = suites.setdefault(run["suite"], {"baseline": {"valid": 0, "invalid": 0}, "candidate": {"valid": 0, "invalid": 0}})
        if run.get("server_exited"):
            stability[run["side"]] += 1
        if not run.get("valid"):
            s[run["side"]]["invalid"] += 1
            continue
        try:
            metrics = EXTRACT[run["suite"]](run, out)
        except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as e:
            run["valid"] = False
            run.setdefault("invalid_reasons", []).append(f"unreadable result: {e}")
            s[run["side"]]["invalid"] += 1
            continue
        s[run["side"]]["valid"] += 1
        for name, (v, direction, gating) in metrics.items():
            if v is None:
                continue
            values.setdefault(name, {"baseline": [], "candidate": []})[run["side"]].append(float(v))
            meta_metric[name] = (direction, gating)

    rows = []
    for name in sorted(values):
        direction, gating = meta_metric[name]
        base, cand = values[name]["baseline"], values[name]["candidate"]
        if direction == ZERO:
            row = verdict_zero(base, cand)
        elif direction in (LOWER, HIGHER):
            row = verdict_perf(direction, base, cand, a.threshold, a.min_runs, rng)
        else:
            row = {
                "baseline": {"runs": len(base), "median": st.median(base) if base else None},
                "candidate": {"runs": len(cand), "median": st.median(cand) if cand else None},
                "verdict": "INFO",
                "why": "recorded, not compared",
            }
        row.update(metric=name, suite=name.split(".")[0], direction=direction, gating=gating)
        rows.append(row)

    stab = verdict_zero([stability["baseline"]], [stability["candidate"]])
    stab.update(metric="stability.server_exits_during_runs", suite="stability", direction=ZERO, gating=True)
    rows.append(stab)

    suite_verdicts = {}
    for suite, counts in suites.items():
        gating = [r for r in rows if r["suite"] == suite and r["gating"]]
        if counts["baseline"]["valid"] == 0 or counts["candidate"]["valid"] == 0:
            v = "INCONCLUSIVE"
        elif any(r["verdict"] == "FAIL" for r in gating):
            v = "FAIL"
        elif any(r["verdict"] == "INCONCLUSIVE" for r in gating):
            v = "INCONCLUSIVE"
        else:
            v = "PASS"
        suite_verdicts[suite] = {"verdict": v, "runs": counts}
    suite_verdicts["stability"] = {"verdict": stab["verdict"], "runs": stability}

    verdicts = [v["verdict"] for v in suite_verdicts.values()]
    overall = "FAIL" if "FAIL" in verdicts else "INCONCLUSIVE" if ("INCONCLUSIVE" in verdicts or not suites) else "PASS"

    summary = {
        "schema": "checkgate.release-benchmark/1",
        "overall": overall,
        "threshold": a.threshold,
        "min_runs": a.min_runs,
        "bootstrap_seed": a.seed,
        "baseline": meta.get("baseline"),
        "candidate": meta.get("candidate"),
        "host": meta.get("host"),
        "suites": suite_verdicts,
        "metrics": rows,
        "invalid_runs": [
            {k: r.get(k) for k in ("suite", "side", "round", "seq", "invalid_reasons", "log")}
            for r in runs
            if not r.get("valid")
        ],
    }
    with open(os.path.join(out, "summary.json"), "w") as f:
        json.dump(summary, f, indent=2)

    md = []
    md.append("# Checkgate v0.2.0 dedicated Linux benchmark\n")
    md.append(f"**Overall: {overall}**  (regression threshold {a.threshold:.0%}, ≥{a.min_runs} valid runs per side, 95 % bootstrap CI, seed {a.seed})\n")
    for side in ("baseline", "candidate"):
        info = meta.get(side) or {}
        md.append(f"- {side}: `{info.get('ref')}` → `{info.get('sha')}`")
    host = meta.get("host") or {}
    if host:
        md.append(f"- host: {host.get('cpu_model')} · {host.get('cpus')} CPUs · {host.get('mem_gib')} GiB · kernel {host.get('kernel')} · governor {host.get('governor')}")
        md.append(f"- server CPUs `{host.get('server_cpuset')}`, load generator CPUs `{host.get('load_cpuset')}`, data stores `{host.get('data_cpuset')}`")
    md.append("")
    md.append("| Suite | Verdict | Valid runs (base / cand) | Invalid (base / cand) |")
    md.append("|---|---|---|---|")
    for suite, v in suite_verdicts.items():
        r = v["runs"]
        if suite == "stability":
            md.append(f"| stability | {v['verdict']} | server exits: {r['baseline']} / {r['candidate']} | |")
        else:
            md.append(f"| {suite} | {v['verdict']} | {r['baseline']['valid']} / {r['candidate']['valid']} | {r['baseline']['invalid']} / {r['candidate']['invalid']} |")
    md.append("")
    md.append("## Metrics\n")
    md.append("| Metric | Gating | Baseline | Candidate | Ratio [95 % CI] | CV base / cand | Verdict | Why |")
    md.append("|---|---|---|---|---|---|---|---|")

    def fmt(x):
        if x is None:
            return "—"
        if isinstance(x, float):
            return f"{x:,.3f}" if abs(x) < 1000 else f"{x:,.0f}"
        return str(x)

    for r in rows:
        b, c = r["baseline"], r["candidate"]
        bv = b.get("median", b.get("total"))
        cvv = c.get("median", c.get("total"))
        ratio = f"{r['ratio']:.3f} [{r['ci95'][0]:.3f}, {r['ci95'][1]:.3f}]" if "ratio" in r else "—"
        cvs = f"{b.get('cv', 0):.1%} / {c.get('cv', 0):.1%}" if "cv" in b else "—"
        md.append(f"| `{r['metric']}` | {'yes' if r['gating'] else 'no'} | {fmt(bv)} ({b['runs']}) | {fmt(cvv)} ({c['runs']}) | {ratio} | {cvs} | {r['verdict']} | {r['why']} |")
    inv = summary["invalid_runs"]
    md.append("\n## Invalid runs\n")
    if inv:
        for r in inv:
            md.append(f"- {r['suite']} {r['side']} round {r['round']} (#{r['seq']}): {'; '.join(r.get('invalid_reasons') or ['?'])} — log `{r.get('log')}`")
    else:
        md.append("None.")
    md.append("\nMethod: see the docstring of `scripts/release-benchmarks/lib/analyze.py`; raw data in `raw/`, logs in `logs/`, environment in `environment.json`.")
    with open(os.path.join(out, "summary.md"), "w") as f:
        f.write("\n".join(md) + "\n")
    print(f"overall: {overall}")
    for suite, v in suite_verdicts.items():
        print(f"  {suite}: {v['verdict']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
