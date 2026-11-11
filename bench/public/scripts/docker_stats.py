#!/usr/bin/env python3
"""Summarize a `docker stats --format '{{json .}}'` capture per container.

    docker_stats.py stats.jsonl > run.containers.json

The capture covers the whole repetition (seeding, warm-up, pauses and
measurement), so these figures locate bottlenecks (which container ran hot)
rather than give per-window numbers; the harnesses report the server's own
per-window CPU and RSS from /proc. CPU is in percent of one CPU.
"""

import json
import re
import sys

UNITS = {"B": 1, "KiB": 1024, "MiB": 1024 ** 2, "GiB": 1024 ** 3,
         "kB": 1000, "MB": 1000 ** 2, "GB": 1000 ** 3}


def to_mib(text):
    m = re.match(r"\s*([\d.]+)\s*([A-Za-z]+)", text)
    if not m:
        return None
    return float(m.group(1)) * UNITS.get(m.group(2), 1) / 1024 ** 2


def pct(values, p):
    s = sorted(values)
    return s[round((len(s) - 1) * p)] if s else None


def main():
    per = {}
    with open(sys.argv[1], encoding="utf-8", errors="replace") as f:
        for line in f:
            # docker stats wraps each line in terminal escapes (ESC[H … ESC[K).
            start, end = line.find("{"), line.rfind("}")
            if start < 0 or end < start:
                continue
            try:
                rec = json.loads(line[start:end + 1])
            except ValueError:
                continue
            name = rec.get("Name", "?")
            if name == "--":  # a container that is being removed
                continue
            # Compose names containers <project>-<service>-<n>; keep the service.
            service = re.sub(r"^checkgate-bench-|-\d+$", "", name)
            service = re.sub(r"-run-[0-9a-f]+$", "", service)
            entry = per.setdefault(service, {"cpu": [], "mem": []})
            try:
                entry["cpu"].append(float(rec.get("CPUPerc", "0").rstrip("%")))
            except ValueError:
                pass
            mem = to_mib(rec.get("MemUsage", "").split("/")[0])
            if mem is not None:
                entry["mem"].append(mem)
    out = {}
    for service, e in sorted(per.items()):
        out[service] = {
            "samples": len(e["cpu"]),
            "cpu_pct_p50": round(pct(e["cpu"], 0.5) or 0, 1),
            "cpu_pct_p90": round(pct(e["cpu"], 0.9) or 0, 1),
            "cpu_pct_max": round(max(e["cpu"], default=0), 1),
            "mem_mib_max": round(max(e["mem"], default=0), 1),
        }
    json.dump({"source": "docker stats, ~1 s samples over the whole repetition",
               "containers": out}, sys.stdout, indent=2)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
