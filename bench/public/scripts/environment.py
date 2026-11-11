#!/usr/bin/env python3
"""Record the benchmark environment as JSON.

    environment.py bench/public/compose.yml > environment.json

Host hardware and OS, Docker and its VM, toolchains, the commit, whether the
measured source (core/, server/src/, sdks/nodejs/) differs from that commit,
the pinned images, and each service's resource limits from the compose file.
"""

import json
import os
import platform
import subprocess
import sys


def sh(*cmd, default=None):
    try:
        return subprocess.run(cmd, capture_output=True, text=True, check=True, timeout=120).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return default


def host():
    if sys.platform == "darwin":
        return {
            "cpu": sh("sysctl", "-n", "machdep.cpu.brand_string"),
            "physical_cores": int(sh("sysctl", "-n", "hw.physicalcpu", default="0")),
            "logical_cpus": int(sh("sysctl", "-n", "hw.logicalcpu", default="0")),
            "memory_gib": round(int(sh("sysctl", "-n", "hw.memsize", default="0")) / 1024 ** 3, 1),
            "os": f"macOS {sh('sw_vers', '-productVersion')}",
            "kernel": platform.release(),
            "arch": platform.machine(),
        }
    cpu = None
    with open("/proc/cpuinfo") as f:
        for line in f:
            if line.startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                break
    with open("/proc/meminfo") as f:
        mem_kb = int(f.readline().split()[1])
    return {
        "cpu": cpu,
        "logical_cpus": os.cpu_count(),
        "memory_gib": round(mem_kb / 1024 ** 2, 1),
        "os": sh("sh", "-c", ". /etc/os-release && echo \"$PRETTY_NAME\""),
        "kernel": platform.release(),
        "arch": platform.machine(),
    }


def main():
    compose_file = sys.argv[1]
    root = sh("git", "rev-parse", "--show-toplevel")
    cfg = json.loads(sh("docker", "compose", "-f", compose_file, "--profile", "tools",
                        "config", "--format", "json", default="{}"))
    services = {}
    for name, svc in sorted(cfg.get("services", {}).items()):
        mem = svc.get("mem_limit")
        services[name] = {
            "image": svc.get("image"),
            "cpus": svc.get("cpus"),
            "memory_mib": round(int(mem) / 1024 ** 2) if str(mem or "").isdigit() else mem,
        }
    images = {}
    for name in ("postgres", "redis"):
        ref = services.get(name, {}).get("image")
        if ref:
            images[name] = ref
    measured = ["core", "server/src", "sdks/nodejs/src", "sdks/nodejs/index.js"]
    dirty = sh("git", "-C", root, "status", "--porcelain", "--", *measured, default="")
    docker_info = json.loads(sh("docker", "info", "--format", "{{json .}}", default="{}"))
    out = {
        "recorded_utc": sh("date", "-u", "+%Y-%m-%dT%H:%M:%SZ"),
        "host": host(),
        "docker": {
            "server_version": docker_info.get("ServerVersion"),
            "operating_system": docker_info.get("OperatingSystem"),
            "vm_cpus": docker_info.get("NCPU"),
            "vm_memory_gib": round((docker_info.get("MemTotal") or 0) / 1024 ** 3, 1),
            "kernel": docker_info.get("KernelVersion"),
            "compose": sh("docker", "compose", "version", "--short"),
        },
        "toolchains": {
            "rustc": sh("rustc", "--version"),
            "node_host": sh("node", "--version"),
            "node_container": "22.23.4 (node:22.23.4-bookworm-slim, pinned in bench/public/Dockerfile)",
        },
        "services_versions": {
            "postgres": sh("docker", "run", "--rm", images.get("postgres", "postgres:16"), "postgres", "--version"),
            "redis": sh("docker", "run", "--rm", images.get("redis", "redis:7"), "redis-server", "--version"),
        },
        "git": {
            "commit": sh("git", "-C", root, "rev-parse", "HEAD"),
            "branch": sh("git", "-C", root, "rev-parse", "--abbrev-ref", "HEAD"),
            "measured_source_matches_commit": dirty == "",
        },
        "services": services,
    }
    json.dump(out, sys.stdout, indent=2)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
