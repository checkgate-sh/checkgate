#!/usr/bin/env python3
"""Per-second resource sampler for scripts/release-benchmarks/v0.2.0-linux.sh.

    sampler.py --pid PID --port PORT --stop-file PATH --out samples.jsonl

One JSON line per second until --stop-file exists or the process is gone:
server CPU seconds (user+system), RSS and its high-water mark, threads, open
file descriptors and established TCP connections to --port, plus host load,
busy and steal CPU share and swap in use. Linux only (reads /proc). The last
line is a summary ({"summary": ...}), including whether the process exited
while it was being sampled.
"""

import argparse
import json
import os
import time

TICK = os.sysconf("SC_CLK_TCK")


def proc_cpu_seconds(pid):
    with open(f"/proc/{pid}/stat") as f:
        fields = f.read().rsplit(")", 1)[1].split()
    # fields[0] is state (field 3); utime/stime are fields 14/15.
    return (int(fields[11]) + int(fields[12])) / TICK


def proc_status(pid):
    out = {}
    with open(f"/proc/{pid}/status") as f:
        for line in f:
            k, _, v = line.partition(":")
            if k in ("VmRSS", "VmHWM"):
                out[k] = int(v.split()[0]) / 1024  # MiB
            elif k == "Threads":
                out[k] = int(v)
    return out


def fd_count(pid):
    try:
        return len(os.listdir(f"/proc/{pid}/fd"))
    except OSError:
        return None


def established(port):
    """ESTABLISHED sockets whose local port is `port` (server side)."""
    n = 0
    hexport = f"{port:04X}"
    for path in ("/proc/net/tcp", "/proc/net/tcp6"):
        try:
            with open(path) as f:
                next(f)
                for line in f:
                    parts = line.split()
                    if parts[1].rsplit(":", 1)[1] == hexport and parts[3] == "01":
                        n += 1
        except OSError:
            pass
    return n


def host_cpu():
    with open("/proc/stat") as f:
        v = [int(x) for x in f.readline().split()[1:]]
    idle = v[3] + v[4]
    steal = v[7] if len(v) > 7 else 0
    return sum(v), idle, steal


def swap_used_mib():
    total = free = 0
    with open("/proc/meminfo") as f:
        for line in f:
            if line.startswith("SwapTotal:"):
                total = int(line.split()[1])
            elif line.startswith("SwapFree:"):
                free = int(line.split()[1])
    return (total - free) / 1024


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pid", type=int, required=True)
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--stop-file", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--interval", type=float, default=1.0)
    a = ap.parse_args()

    start = time.monotonic()
    prev_host = host_cpu()
    prev_cpu = None
    peak = {"rss_mib": 0.0, "hwm_mib": 0.0, "threads": 0, "fds": 0, "connections": 0}
    cpu_first = cpu_last = None
    busy = []
    steal = []
    swap0 = swap_used_mib()
    swap_max = swap0
    exited = False
    with open(a.out, "w") as out:
        while not os.path.exists(a.stop_file):
            try:
                cpu = proc_cpu_seconds(a.pid)
                st = proc_status(a.pid)
            except (OSError, IndexError):
                exited = True
                break
            now = time.monotonic()
            host = host_cpu()
            dt_total = host[0] - prev_host[0]
            busy_share = 1 - (host[1] - prev_host[1]) / dt_total if dt_total else 0.0
            steal_share = (host[2] - prev_host[2]) / dt_total if dt_total else 0.0
            prev_host = host
            cores = None
            if prev_cpu is not None:
                cores = (cpu - prev_cpu[0]) / (now - prev_cpu[1])
            prev_cpu = (cpu, now)
            cpu_first = cpu if cpu_first is None else cpu_first
            cpu_last = cpu
            fds = fd_count(a.pid)
            conns = established(a.port)
            swap = swap_used_mib()
            swap_max = max(swap_max, swap)
            busy.append(busy_share)
            steal.append(steal_share)
            row = {
                "t": round(now - start, 3),
                "cpu_s": round(cpu, 3),
                "cpu_cores": None if cores is None else round(cores, 4),
                "rss_mib": round(st.get("VmRSS", 0), 2),
                "hwm_mib": round(st.get("VmHWM", 0), 2),
                "threads": st.get("Threads"),
                "fds": fds,
                "connections": conns,
                "load1": float(open("/proc/loadavg").read().split()[0]),
                "host_busy": round(busy_share, 4),
                "host_steal": round(steal_share, 4),
                "swap_used_mib": round(swap, 1),
            }
            out.write(json.dumps(row) + "\n")
            out.flush()
            peak["rss_mib"] = max(peak["rss_mib"], row["rss_mib"])
            peak["hwm_mib"] = max(peak["hwm_mib"], row["hwm_mib"])
            peak["threads"] = max(peak["threads"], row["threads"] or 0)
            peak["fds"] = max(peak["fds"], fds or 0)
            peak["connections"] = max(peak["connections"], conns)
            time.sleep(a.interval)
        elapsed = time.monotonic() - start
        summary = {
            "seconds": round(elapsed, 3),
            "process_exited": exited,
            "server_cpu_s": None if cpu_first is None else round(cpu_last - cpu_first, 3),
            "server_cpu_cores_mean": None
            if cpu_first is None or elapsed <= 0
            else round((cpu_last - cpu_first) / elapsed, 4),
            "peak": peak,
            "host_busy_mean": round(sum(busy) / len(busy), 4) if busy else None,
            "host_steal_max": round(max(steal), 4) if steal else None,
            "swap_growth_mib": round(swap_max - swap0, 1),
        }
        out.write(json.dumps({"summary": summary}) + "\n")


if __name__ == "__main__":
    main()
