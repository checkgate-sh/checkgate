#!/usr/bin/env python3
"""Release check for the Python SDK's wheels, without publishing anything.

    python3 scripts/check-python-package.py DIST_DIR [--version X] [--json OUT]

For every wheel in DIST_DIR (as built by release.yml / python-wheels.yml):

- filename: distribution `checkgate`, the expected version, a CPython abi3
  tag for 3.11 (`cp311-abi3`), and a platform tag (manylinux/musllinux,
  macosx or win);
- METADATA: Name, Version, Requires-Python >= 3.11, License-Expression
  Apache-2.0, a License-File, a summary and project URLs;
- contents: the native module (`checkgate/_core.abi3.so`, or `_core.pyd` on
  Windows), `__init__.py`, `py.typed`, `_core.pyi`, the license; no tests,
  examples or caches;
- integrity: every file's SHA-256 and size match RECORD, and RECORD lists
  every file;
- version consistency: the wheel version, `checkgate.__version__` inside the
  wheel and sdks/python/pyproject.toml agree (and match --version if given).

Exit status 1 on any failure. Standard library only.
"""

import argparse
import base64
import csv
import hashlib
import io
import json
import os
import re
import sys
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def pyproject_version():
    text = open(os.path.join(ROOT, "sdks/python/pyproject.toml"), encoding="utf-8").read()
    m = re.search(r'^version\s*=\s*"([^"]+)"', text, re.M)
    return m.group(1) if m else None


def check_wheel(path, expected):
    problems, facts = [], {}
    name = os.path.basename(path)
    m = re.match(r"^(?P<dist>[^-]+)-(?P<ver>[^-]+)-(?P<py>[^-]+)-(?P<abi>[^-]+)-(?P<plat>.+)\.whl$", name)
    if not m:
        return [f"{name}: not a wheel filename"], facts
    facts.update(m.groupdict())
    if m["dist"] != "checkgate":
        problems.append(f"distribution is {m['dist']!r}, not 'checkgate'")
    if expected and m["ver"] != expected:
        problems.append(f"filename version {m['ver']} != expected {expected}")
    if m["py"] != "cp311" or m["abi"] != "abi3":
        problems.append(f"tags {m['py']}-{m['abi']}: expected cp311-abi3 (one wheel for CPython 3.11+)")
    if not re.match(r"^(manylinux|musllinux|macosx|win)", m["plat"]):
        problems.append(f"platform tag {m['plat']!r} is not publishable (linux_* wheels are refused by PyPI; build with manylinux)")

    with zipfile.ZipFile(path) as z:
        names = z.namelist()
        dist_info = f"checkgate-{m['ver']}.dist-info"
        meta_text = z.read(f"{dist_info}/METADATA").decode("utf-8")
        meta = {}
        for line in meta_text.split("\n\n", 1)[0].splitlines():
            k, _, v = line.partition(": ")
            meta.setdefault(k, []).append(v)
        facts["metadata"] = {k: v if len(v) > 1 else v[0] for k, v in meta.items()}
        get = lambda k: (meta.get(k) or [None])[0]  # noqa: E731
        if get("Name") != "checkgate":
            problems.append(f"METADATA Name {get('Name')!r}")
        if get("Version") != m["ver"]:
            problems.append(f"METADATA Version {get('Version')} != filename {m['ver']}")
        if (get("Requires-Python") or "").replace(" ", "") != ">=3.11":
            problems.append(f"Requires-Python {get('Requires-Python')!r}, expected '>=3.11'")
        if get("License-Expression") != "Apache-2.0":
            problems.append(f"License-Expression {get('License-Expression')!r}")
        if not meta.get("License-File"):
            problems.append("no License-File in METADATA")
        if not get("Summary"):
            problems.append("no Summary")
        if not meta.get("Project-URL"):
            problems.append("no Project-URL")
        if get("Metadata-Version") not in ("2.3", "2.4", "2.5"):
            problems.append(f"Metadata-Version {get('Metadata-Version')} (License-Expression needs 2.4)")

        native = [n for n in names if re.match(r"^checkgate/_core(\.abi3\.so|\.pyd)$", n)]
        if len(native) != 1:
            problems.append(f"native module: found {native or 'none'}")
        for must in ("checkgate/__init__.py", "checkgate/py.typed", "checkgate/_core.pyi"):
            if must not in names:
                problems.append(f"missing {must}")
        if not any(n.startswith(f"{dist_info}/licenses/") for n in names):
            problems.append("no license file under dist-info/licenses/")
        bad = [n for n in names if re.search(r"(^|/)(tests?|examples?|__pycache__)/|\.pyc$", n)]
        if bad:
            problems.append(f"files that do not belong in a wheel: {bad[:5]}")

        record_name = f"{dist_info}/RECORD"
        rows = list(csv.reader(io.StringIO(z.read(record_name).decode("utf-8"))))
        recorded = {r[0]: r for r in rows if r}
        for n in names:
            if n.endswith("/"):
                continue
            if n not in recorded:
                problems.append(f"{n} is not in RECORD")
                continue
            if n == record_name:
                continue
            _, digest, size = recorded[n]
            data = z.read(n)
            want = "sha256=" + base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
            if digest != want or int(size) != len(data):
                problems.append(f"{n}: RECORD hash/size mismatch")
        for n in recorded:
            if n not in names:
                problems.append(f"RECORD lists {n}, which the wheel lacks")

        client = z.read("checkgate/_client.py").decode("utf-8") if "checkgate/_client.py" in names else ""
        mv = re.search(r'^__version__\s*=\s*"([^"]+)"', client, re.M)
        facts["module_version"] = mv.group(1) if mv else None
        if facts["module_version"] != m["ver"]:
            problems.append(f"checkgate.__version__ {facts['module_version']} != wheel {m['ver']}")
        facts["native_module"] = native[0] if native else None
        facts["native_bytes"] = z.getinfo(native[0]).file_size if native else None
        facts["files"] = len(names)
    facts["sha256"] = hashlib.sha256(open(path, "rb").read()).hexdigest()
    return problems, facts


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dist")
    ap.add_argument("--version")
    ap.add_argument("--json")
    ap.add_argument("--allow-local-platform", action="store_true",
                    help="accept linux_* platform tags (a local, non-manylinux build)")
    a = ap.parse_args()
    expected = a.version or pyproject_version()
    wheels = sorted(f for f in os.listdir(a.dist) if f.endswith(".whl"))
    if not wheels:
        print(f"no wheels in {a.dist}")
        return 1
    report, failed = {"expected_version": expected, "pyproject_version": pyproject_version(), "wheels": {}}, False
    if a.version and pyproject_version() != a.version:
        print(f"FAIL pyproject.toml version {pyproject_version()} != --version {a.version}")
        failed = True
    for w in wheels:
        problems, facts = check_wheel(os.path.join(a.dist, w), expected)
        if a.allow_local_platform:
            problems = [p for p in problems if not p.startswith("platform tag 'linux_")]
        report["wheels"][w] = {"problems": problems, **facts}
        if problems:
            failed = True
            print(f"FAIL {w}")
            for p in problems:
                print(f"     - {p}")
        else:
            print(f"ok   {w} ({facts['files']} files, {facts['native_module']} {facts['native_bytes']} bytes, sha256 {facts['sha256'][:16]}…)")
    if a.json:
        with open(a.json, "w") as f:
            json.dump(report, f, indent=2)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
