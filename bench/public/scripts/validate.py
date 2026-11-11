#!/usr/bin/env python3
"""Fast checks of the published benchmark results and graphic (runs in CI).

    python3 bench/public/scripts/validate.py

Does not run any benchmark. Checks that:
1. summary.json and every raw run file parse and carry the expected fields;
   the summary was generated from the raw files now on disk.
2. Every headline number in the graphic comes from summary.json (the SVGs
   are re-rendered and must be byte-identical — generation is deterministic
   and the committed files are current).
3. The SVGs are well-formed XML with <title> and <desc>, and contain no
   script, external reference, event handler or web font.
4. The README states the headline numbers (alt text); every relative link and image in the README's Performance section, and the
   benchmark docs, resolves to a file in the repository.
5. Every `bench/public/run.sh <command>` written in the docs is a real
   command, and every repository path the docs name exists.
"""

import json
import os
import re
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET

sys.dont_write_bytecode = True  # importing render_svg must not leave __pycache__ in the repo

ROOT = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True,
                      text=True, check=True).stdout.strip()
HERE = os.path.join(ROOT, "bench", "public")
RESULTS = os.path.join(ROOT, "docs", "performance", "results", "v0.1.30")
ASSETS = os.path.join(ROOT, "assets", "benchmarks")
DOCS = [os.path.join(ROOT, "docs", "performance", "benchmarks.md"), os.path.join(HERE, "README.md")]
errors = []


def fail(msg):
    errors.append(msg)


def load(path):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError) as e:
        fail(f"{os.path.relpath(path, ROOT)}: {e}")
        return None


def require(obj, path, keys, where):
    for k in keys:
        if not isinstance(obj, dict) or k not in obj:
            fail(f"{where}: missing `{path}{k}`")


def is_num(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def check_schema():
    s = load(os.path.join(RESULTS, "summary.json"))
    env = load(os.path.join(RESULTS, "environment.json"))
    if s is None or env is None:
        return None
    require(s, "", ["schema", "version", "commit", "date", "environment", "runs", "failed_run_files",
                    "evaluation", "throughput", "propagation", "headline", "repetitions"], "summary.json")
    if s.get("schema") != "checkgate-benchmarks/v1":
        fail(f"summary.json: unexpected schema {s.get('schema')!r}")
    if not re.fullmatch(r"[0-9a-f]{40}", str(s.get("commit"))):
        fail("summary.json: commit is not a full SHA")
    for key, h in (s.get("headline") or {}).items():
        require(h, f"headline.{key}.", ["value", "unit", "scenario", "context", "source"], "summary.json")
        if not is_num(h.get("value")) or h["value"] <= 0:
            fail(f"summary.json: headline.{key}.value is not a positive number")
    raw = os.path.join(RESULTS, "raw")
    for name in sorted(os.listdir(raw)):
        path = os.path.join(raw, name)
        if not name.endswith(".json"):
            continue
        d = load(path)
        if d is None:
            continue
        if name.startswith("evaluation-rust-run"):
            require(d, "", ["benchmarks", "allocations_per_evaluation"], name)
            for bid, b in d.get("benchmarks", {}).items():
                require(b, f"{bid}.", ["estimate_ns", "estimate_ci95_ns", "samples"], name)
        elif name.startswith("evaluation-node-run"):
            require(d, "", ["cases", "checks"], name)
            if (d.get("checks") or {}).get("network_calls_during_timing") != 0:
                fail(f"{name}: network calls during timing")
        elif name.startswith("throughput-run") and not name.endswith(".containers.json"):
            require(d, "", ["endpoint", "levels", "response_bytes"], name)
            for lv in d.get("levels", []):
                require(lv, "level.", ["concurrency", "successful_rps", "failed_requests", "latency", "server"], name)
        elif name.startswith("propagation-run") and not name.endswith(".containers.json"):
            require(d, "", ["profiles"], name)
            for p in d.get("profiles", []):
                require(p.get("propagation", {}), "propagation.",
                        ["missed", "visible_from_request", "visible_from_request_histogram_0_1ms"], name)
    # The summary must be what summarize.py makes from these files today.
    regenerated = subprocess.run([sys.executable, os.path.join(HERE, "scripts", "summarize.py"), RESULTS],
                                 capture_output=True, text=True)
    if regenerated.returncode != 0:
        fail(f"summarize.py failed: {regenerated.stderr.strip()}")
    elif json.loads(regenerated.stdout) != s:
        fail("summary.json is stale: rerun `bench/public/run.sh summarize`")
    return s


def check_svgs(summary):
    with tempfile.TemporaryDirectory() as tmp:
        tmp_summary = os.path.join(tmp, "summary.json")
        with open(tmp_summary, "w", encoding="utf-8") as f:
            json.dump(summary, f)
        out = os.path.join(tmp, "svg")
        r = subprocess.run([sys.executable, os.path.join(HERE, "scripts", "render_svg.py"), tmp_summary, out],
                           capture_output=True, text=True)
        if r.returncode != 0:
            fail(f"render_svg.py failed: {r.stderr.strip()}")
            return
        for theme in ("light", "dark"):
            name = f"performance-{theme}.svg"
            committed = os.path.join(ASSETS, name)
            if not os.path.exists(committed):
                fail(f"assets/benchmarks/{name} is missing")
                continue
            with open(committed, "rb") as a, open(os.path.join(out, name), "rb") as b:
                body, fresh = a.read(), b.read()
            if body != fresh:
                fail(f"assets/benchmarks/{name} differs from a fresh render of summary.json")
            # Rendering twice must give the same bytes.
            subprocess.run([sys.executable, os.path.join(HERE, "scripts", "render_svg.py"),
                            tmp_summary, out + "2"], capture_output=True, check=False)
            with open(os.path.join(out + "2", name), "rb") as c:
                if c.read() != fresh:
                    fail(f"{name}: two renders of the same summary differ")
            check_svg_content(name, body)


def check_svg_content(name, body):
    if len(body) > 40_000:
        fail(f"{name}: {len(body)} bytes, over the 40 kB budget")
    try:
        root = ET.fromstring(body)
    except ET.ParseError as e:
        fail(f"{name}: not well-formed XML: {e}")
        return
    ns = "{http://www.w3.org/2000/svg}"
    if root.tag != f"{ns}svg" or not root.get("viewBox"):
        fail(f"{name}: root is not an <svg> with a viewBox")
    for tag in ("title", "desc"):
        el = root.find(f"{ns}{tag}")
        if el is None or not (el.text or "").strip():
            fail(f"{name}: missing <{tag}>")
    text = body.decode("utf-8")
    for pattern, why in ((r"<script", "script"), (r"\son[a-z]+=", "event handler"),
                         (r"(href|src)=\"(?!#)", "external reference"), (r"@import|@font-face|url\(", "web font or url()"),
                         (r"<image", "embedded image"), (r"<foreignObject", "foreignObject")):
        if re.search(pattern, text, re.I):
            fail(f"{name}: contains {why}")


def links(md_path, section=None):
    with open(md_path, encoding="utf-8") as f:
        text = f.read()
    if section:
        m = re.search(rf"^## {re.escape(section)}\n(.*?)(?=^## |\Z)", text, re.M | re.S)
        if not m:
            fail(f"{os.path.relpath(md_path, ROOT)}: no `## {section}` section")
            return []
        text = m.group(1)
    found = re.findall(r"\]\(([^)\s]+)\)", text) + re.findall(r'(?:src|srcset)="([^"]+)"', text)
    return [l for l in found if not re.match(r"[a-z]+:", l) and not l.startswith("#")]


def check_links():
    targets = [(os.path.join(ROOT, "README.md"), "Performance")] + [(d, None) for d in DOCS]
    for md, section in targets:
        if not os.path.exists(md):
            fail(f"{os.path.relpath(md, ROOT)} is missing")
            continue
        for link in links(md, section):
            path = link.split("#")[0]
            base = ROOT if path.startswith("/") else os.path.dirname(md)
            resolved = os.path.normpath(os.path.join(base, path.lstrip("/")))
            if not os.path.exists(resolved):
                fail(f"{os.path.relpath(md, ROOT)}: link `{link}` does not resolve")


def check_readme_numbers(summary):
    """The README's alt text repeats the headline numbers for screen readers;
    they must match the graphic, so a re-run cannot leave them stale."""
    sys.path.insert(0, os.path.join(HERE, "scripts"))
    from render_svg import fmt_value  # the graphic's own formatting
    with open(os.path.join(ROOT, "README.md"), encoding="utf-8") as f:
        m = re.search(r"^## Performance\n(.*?)(?=^## |\Z)", f.read(), re.M | re.S)
    section = m.group(1) if m else ""
    for key, h in summary.get("headline", {}).items():
        num, unit = fmt_value(h)
        if f"{num} {unit}" not in section and f"{num} {h['unit']}" not in section and num not in section:
            fail(f"README.md Performance section does not state the {key} headline ({num} {unit})")


def check_commands():
    with open(os.path.join(HERE, "run.sh"), encoding="utf-8") as f:
        run_sh = f.read()
    commands = set(re.findall(r"^\s+([a-z]+)\)\s", run_sh, re.M))
    for md in DOCS:
        if not os.path.exists(md):
            continue
        with open(md, encoding="utf-8") as f:
            text = f.read()
        for cmd in re.findall(r"bench/public/run\.sh ([a-z]+)", text):
            if cmd not in commands:
                fail(f"{os.path.relpath(md, ROOT)}: `run.sh {cmd}` is not a run.sh command")
        for path in set(re.findall(r"`((?:bench|server|docs|assets|sdks)/[\w./-]+\.\w+)", text)):
            if not os.path.exists(os.path.join(ROOT, path)):
                fail(f"{os.path.relpath(md, ROOT)}: names `{path}`, which does not exist")


def main():
    summary = check_schema()
    if summary is not None:
        check_svgs(summary)
        check_readme_numbers(summary)
    check_links()
    check_commands()
    if errors:
        for e in errors:
            print(f"error: {e}", file=sys.stderr)
        sys.exit(1)
    print("ok: benchmark results, graphic, links and documented commands are consistent")


if __name__ == "__main__":
    main()
