#!/usr/bin/env python3
"""Aggregate the connected-client cost runs into cost-summary.json.

    summarize_cost.py docs/performance/results/v0.1.31 > .../cost-summary.json

Inputs: raw/cost-*-run*.json (sdk/connections.mjs), their .containers.json
(docker stats), and bench/public/pricing.json. Deterministic.

Four kinds of number, kept apart:
- measured: read from the runs (median over repetitions when there are
  several; a profile counts as sustained only if it was in every run).
- calculated: measured per-event costs times a month of events, using the
  stated assumptions (flag changes a day, reconnects a day).
- pricing: inputs from pricing.json, with sources and the access date.
- projected: calculated usage priced with those inputs, for profiles that
  were sustained on one server under the published limits. Profiles that
  were not sustained get no price: what they would need was not measured.

A polling profile is "sustained" when it reached at least 95 % of its
target request rate with no errors; an SSE profile when every client
connected, every update reached every client, and every client reconnected.
"""

import glob
import json
import os
import re
import statistics
import sys

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def load(path):
    with open(path) as f:
        return json.load(f)


def med(values):
    values = [v for v in values if v is not None]
    return round(statistics.median(values), 3) if values else None


def r3(v):
    return None if v is None else round(v, 3)


def profile_runs(raw):
    groups = {}
    for path in sorted(glob.glob(os.path.join(raw, "cost-*-run*.json"))):
        if path.endswith(".containers.json"):
            continue
        name = os.path.basename(path)
        m = re.match(r"cost-(sse|poll)-(\d+)(?:-(\d+)s)?-run(\d+)\.json$", name)
        if not m:
            continue
        key = (m.group(1), int(m.group(2)), int(m.group(3)) if m.group(3) else None)
        stats_path = path[:-5] + ".containers.json"
        stats = load(stats_path)["containers"] if os.path.exists(stats_path) else {}
        groups.setdefault(key, []).append((load(path), stats))
    failed = {}
    for path in sorted(glob.glob(os.path.join(raw, "failed", "cost-*-run*.json"))):
        if path.endswith(".containers.json"):
            continue
        m = re.match(r"cost-(sse|poll)-(\d+)(?:-(\d+)s)?-run(\d+)\.json$", os.path.basename(path))
        if m:
            key = (m.group(1), int(m.group(2)), int(m.group(3)) if m.group(3) else None)
            failed.setdefault(key, []).append(load(path))
    return groups, failed


def container(stats_list, svc, field):
    return med([s.get(svc, {}).get(field) for s in stats_list])


def sse_profile(n, runs, failed):
    w = [r["windows"] for r, _ in runs]
    stats = [s for _, s in runs]
    ok = (not failed) and all(
        x.get("ramp", {}).get("all_ready") and x.get("updates", {}).get("missed") == 0
        and x.get("reconnect", {}).get("all_ready") for x in w)
    idle = [x["idle"] for x in w if "idle" in x]
    upd = [x["updates"] for x in w if "updates" in x]
    rec = [x["reconnect"] for x in w if "reconnect" in x]
    ramp = [x["ramp"] for x in w if "ramp" in x]
    return {
        "clients": n, "runs": len(runs), "failed_runs": len(failed), "sustained": bool(ok),
        "errors": sorted({f.get("error") for f in failed if f.get("error")}),
        "ramp": {
            "seconds_to_all_ready": med([x["secs"] for x in ramp]),
            "refused_attempts": med([x["refused_attempts"] for x in ramp]),
            "server_reported_connections": med([x.get("server_reported_connections") for x in ramp]),
            "bootstrap_bytes_per_client": med([x["bootstrap_bytes"] / n for x in ramp]),
        },
        "idle": {
            "server_cpu_pct": med([x["server_cpu_pct"] for x in idle]),
            "server_rss_mib": med([x["server_rss_mib"] for x in idle]),
            "server_net_tx_bytes_per_s": med([x["net_tx_bytes"] / x["secs"] for x in idle if x["net_tx_bytes"] is not None]),
            "server_net_rx_bytes_per_s": med([x["net_rx_bytes"] / x["secs"] for x in idle if x["net_rx_bytes"] is not None]),
            "heartbeat_bytes_per_client_per_hour": med([x["heartbeat_bytes_per_client_per_hour"] for x in idle]),
        },
        "updates": {
            "updates": med([x["updates"] for x in upd]),
            "deliveries": med([x["deliveries"] for x in upd]),
            "missed": max([x["missed"] for x in upd], default=None),
            "delivery_p50_ms": med([x["delivery_ms"].get("p50_ms") for x in upd]),
            "delivery_p99_ms": med([x["delivery_ms"].get("p99_ms") for x in upd]),
            "server_cpu_pct": med([x["server_cpu_pct"] for x in upd]),
            "update_bytes_per_client_per_update": med([x["update_bytes_per_client_per_update"] for x in upd]),
            "timing": "PATCH sent → the event read from the client's socket (raw client, not an SDK)",
        },
        "reconnect": {
            "seconds_to_all_ready": med([x["secs"] for x in rec]),
            "includes_client_wait_s": "3.0-3.9",
            "refused_attempts": med([x["refused_attempts"] for x in rec]),
            "server_cpu_pct": med([x["server_cpu_pct"] for x in rec]),
            "rebootstrap_bytes_per_client": med([x["rebootstrap_bytes_per_client"] for x in rec]),
        },
        "server_rss_peak_mib": med([max(x[k]["server_rss_peak_mib"] for k in ("ramp", "idle", "updates", "reconnect") if k in x) for x in w]),
        "postgres": {"cpu_pct_p90": container(stats, "postgres", "cpu_pct_p90"), "mem_mib_max": container(stats, "postgres", "mem_mib_max")},
        "redis": {"cpu_pct_p90": container(stats, "redis", "cpu_pct_p90"), "mem_mib_max": container(stats, "redis", "mem_mib_max")},
    }


def poll_profile(n, interval, runs, failed):
    ws = [r["windows"]["poll"] for r, _ in runs if "poll" in r["windows"]]
    stats = [s for _, s in runs]
    sustained = (not failed) and bool(ws) and all(
        x["errors"] == 0 and x["requests_per_s"] >= 0.95 * x["target_requests_per_s"] for x in ws)
    return {
        "clients": n, "interval_s": interval, "runs": len(runs), "failed_runs": len(failed),
        "sustained": bool(sustained),
        "errors": sorted({f.get("error") for f in failed if f.get("error")}),
        "target_requests_per_s": r3(n / interval),
        "requests_per_s": med([x["requests_per_s"] for x in ws]),
        "error_rate": max([x["error_rate"] for x in ws], default=None),
        "server_cpu_pct": med([x["server_cpu_pct"] for x in ws]),
        "server_rss_mib": med([x["server_rss_mib"] for x in ws]),
        "server_rss_peak_mib": med([x["server_rss_peak_mib"] for x in ws]),
        "server_net_tx_bytes_per_s": med([x["net_tx_bytes"] / x["secs"] for x in ws if x["net_tx_bytes"] is not None]),
        "bytes_per_request": med([x["bytes_per_request"] for x in ws]),
        "request_p50_ms": med([x["request_ms"].get("p50_ms") for x in ws]),
        "request_p99_ms": med([x["request_ms"].get("p99_ms") for x in ws]),
        "staleness_p50_ms": med([x["staleness_ms"].get("p50_ms") for x in ws]),
        "staleness_p99_ms": med([x["staleness_ms"].get("p99_ms") for x in ws]),
        "postgres": {"cpu_pct_p90": container(stats, "postgres", "cpu_pct_p90"), "mem_mib_max": container(stats, "postgres", "mem_mib_max")},
    }


def monthly_traffic(sse, polls, pricing):
    a = pricing["assumptions"]
    days = a["days_per_month"]
    out = []
    for p in sse:
        n = p["clients"]
        hb = (p["idle"]["heartbeat_bytes_per_client_per_hour"] or 0) * 24 * days
        upd = (p["updates"]["update_bytes_per_client_per_update"] or 0) * a["flag_changes_per_day"] * days
        boot = (p["reconnect"]["rebootstrap_bytes_per_client"] or 0) * a["stream_reconnects_per_client_per_day"] * days
        per_client = hb + upd + boot
        out.append({
            "mode": "sse", "clients": n,
            "requests_per_month": round(n * a["stream_reconnects_per_client_per_day"] * days),
            "bytes_per_client_per_month": round(per_client),
            "breakdown_bytes_per_client_per_month": {"heartbeats": round(hb), "flag_changes": round(upd), "reconnects": round(boot)},
            "gb_per_month": r3(per_client * n / 1e9),
        })
    for p in polls:
        n, i = p["clients"], p["interval_s"]
        if not p["sustained"]:
            # Bytes per request from a run with errors are not what serving
            # every request would cost; no traffic figure is made from them.
            out.append({"mode": "poll", "clients": n, "interval_s": i, "requests_per_month": None,
                        "bytes_per_client_per_month": None, "gb_per_month": None,
                        "why_none": "not sustained: the measured bytes per request include failed requests"})
            continue
        reqs = n * (86400 / i) * days
        out.append({
            "mode": "poll", "clients": n, "interval_s": i,
            "requests_per_month": round(reqs),
            "bytes_per_client_per_month": round((p["bytes_per_request"] or 0) * (86400 / i) * days),
            "gb_per_month": r3(reqs * (p["bytes_per_request"] or 0) / 1e9),
        })
    return out


def price(gb, pricing):
    it = pricing["items"]
    h = pricing["hours_per_month"]
    fixed = {
        "server": it["server"]["usd_per_hour"] * h,
        "server_disk": it["server_disk"]["usd_per_gb_month"] * it["server_disk"]["gb"],
        "database": it["database"]["usd_per_hour"] * h,
        "database_storage": it["database_storage"]["usd_per_gb_month"] * it["database_storage"]["gb"],
        "redis": it["redis"]["usd_per_hour"] * h,
    }
    egress = max(0.0, gb - it["egress"]["free_gb_per_month"]) * it["egress"]["usd_per_gb"]
    lines = {k: round(v, 2) for k, v in fixed.items()}
    lines["egress"] = round(egress, 2)
    return {"lines_usd": lines, "total_usd": round(sum(fixed.values()) + egress, 2)}


def main():
    out_dir = sys.argv[1]
    raw = os.path.join(out_dir, "raw")
    pricing = load(os.path.join(HERE, "pricing.json"))
    groups, failed = profile_runs(raw)
    keys = sorted(set(groups) | set(failed), key=lambda k: (k[1], k[0] != "sse", k[2] or 0))
    sse, polls = [], []
    for key in keys:
        mode, n, i = key
        runs = groups.get(key, [])
        fails = failed.get(key, [])
        if mode == "sse":
            sse.append(sse_profile(n, runs, fails) if runs else
                       {"clients": n, "runs": 0, "failed_runs": len(fails), "sustained": False,
                        "errors": sorted({f.get("error") for f in fails if f.get("error")})})
        else:
            polls.append(poll_profile(n, i, runs, fails) if runs else
                         {"clients": n, "interval_s": i, "runs": 0, "failed_runs": len(fails), "sustained": False,
                          "errors": sorted({f.get("error") for f in fails if f.get("error")})})
    measured_sse = [p for p in sse if p.get("runs")]
    measured_poll = [p for p in polls if p.get("runs")]
    traffic = monthly_traffic(measured_sse, measured_poll, pricing)
    projected = []
    for t in traffic:
        prof = next(p for p in (measured_sse if t["mode"] == "sse" else measured_poll)
                    if p["clients"] == t["clients"] and p.get("interval_s") == t.get("interval_s"))
        entry = {k: t[k] for k in ("mode", "clients") if k in t}
        if "interval_s" in t:
            entry["interval_s"] = t["interval_s"]
        if prof["sustained"]:
            entry.update(price(t["gb_per_month"], pricing))
            cpu = prof["idle"]["server_cpu_pct"] if t["mode"] == "sse" else prof["server_cpu_pct"]
            entry["server_cpu_vs_burst_baseline"] = (
                "within" if cpu is not None and cpu <= 40 else "above")
            entry["server_cpu_pct_compared"] = cpu
            entry["server_cpu_basis"] = (
                "idle-stream CPU (flag changes and reconnects are short bursts, paid from CPU credits)"
                if t["mode"] == "sse" else "CPU while polling at the target rate")
        else:
            entry["total_usd"] = None
            entry["why_no_price"] = "not sustained by one server at the published limits; what it would need was not measured"
        projected.append(entry)
    env_path = os.path.join(out_dir, "environment.json")
    env = load(env_path) if os.path.exists(env_path) else {}
    summary = {
        "schema": "checkgate-cost/v1",
        "version": os.path.basename(os.path.normpath(out_dir)).lstrip("v"),
        "commit": env.get("git", {}).get("commit"),
        "limits": "server 1 vCPU / 512 MiB; PostgreSQL 2 vCPU / 1 GiB; Redis 0.5 vCPU / 256 MiB (bench/public/compose.yml)",
        "kinds": {
            "measured": "read from the runs",
            "calculated": "measured per-event costs × a month of events under the stated assumptions",
            "pricing": "inputs from bench/public/pricing.json",
            "projected": "calculated usage priced with those inputs; only for sustained profiles",
        },
        "measured": {"sse": sse, "poll": polls},
        "calculated": {"assumptions": pricing["assumptions"], "monthly_traffic": traffic},
        "pricing": {k: pricing[k] for k in ("accessed", "region", "hours_per_month", "caveat", "items")},
        "projected": {
            "deployment": "one Checkgate server (EC2 t4g.small) + RDS PostgreSQL db.t4g.micro (20 GB) + ElastiCache "
                          "cache.t4g.micro, priced for the client counts that one server sustained under the published limits",
            "monthly": projected,
        },
    }
    json.dump(summary, sys.stdout, indent=2, ensure_ascii=False)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
