#!/usr/bin/env python3
"""Check the built VitePress site against the publication policy.

    (cd docs && npm run build) && python3 scripts/check-docs-site.py [dist-dir]

The docs site is public. Internal engineering records (release reports,
reviews, raw benchmark output, working notes) must never end up in it, whether
as a page, a copied asset, or text in the search index. This check fails when:

1. a file in the output is outside the allowlist below (pages from the public
   sections, VitePress bundles, and image files copied from docs/public/);
2. a page or bundle mentions an internal artifact (internal_docs/, a
   docs/releases/ path other than a commit-pinned GitHub permalink, an
   internal release report by name);
3. a page or bundle contains something that looks like a credential or a
   local filesystem path.

Findings name the file and the rule, never the matched text, so a real secret
is not copied into CI logs.
"""

import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DIST = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "docs", ".vitepress", "dist")

# 1. What may be published. Pages: the home page and top-level pages, and one
#    level of the public sections. Anything deeper (e.g. a README under
#    performance/results/) is not a page of this site.
SECTIONS = ("guide", "sdks", "ecosystem", "operations", "performance")
ALLOWED = [
    re.compile(r"^[a-z0-9-]+\.html$"),
    re.compile(r"^(%s)/[a-z0-9-]+\.html$" % "|".join(SECTIONS)),
    re.compile(r"^hashmap\.json$"),  # VitePress page-hash map
    re.compile(r"^sitemap\.xml$"),  # generated from the public pages (config.ts `sitemap`)
    re.compile(r"^assets/(chunks/)?[A-Za-z0-9@_.-]+\.(js|css|woff2?)$"),
    re.compile(r"^assets/[A-Za-z0-9_.-]+\.(png|jpe?g|webp|gif|svg|ico)$"),  # imported images
    re.compile(r"^[A-Za-z0-9_.-]+\.(png|jpe?g|webp|gif|svg|ico)$"),  # docs/public/ images
]

# 2. References to internal artifacts. A permalink pinned to a commit
#    (github.com/.../blob/<sha>/docs/releases/...) is allowed: it cites
#    history that is already public and does not change. Raw benchmark results
#    (docs/performance/results/) are public reproduction data on GitHub; pages
#    may cite them, and rule 1 keeps the files themselves out of the site.
INTERNAL = {
    "internal_docs path": re.compile(r"internal_docs/"),
    "unpinned docs/releases path": re.compile(r"(?<!/blob/[0-9a-f]{7}/)(?<!/blob/[0-9a-f]{40}/)docs/releases/"),
    "release report name": re.compile(
        r"\b(RC\d-[A-Z0-9-]+|RELEASE-READINESS|SECURITY-REVIEW|BASELINE-ASSESSMENT|"
        r"CORRECTNESS-REVIEW|RELIABILITY-REVIEW|R20-ROOT-CAUSE)\.md\b"),
}

# 3. Credentials and local paths. Placeholders in the docs (sk_live_your_key,
#    pat_your_token, example.com) do not match these shapes.
SECRETS = {
    "AWS access key": re.compile(r"\bAKIA[0-9A-Z]{16}\b"),
    "private key block": re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
    "GitHub token": re.compile(r"\bgh[pousr]_[A-Za-z0-9]{36,}\b"),
    "Slack token": re.compile(r"\bxox[abpr]-[0-9]{6,}-[A-Za-z0-9-]{10,}"),
    "Slack/Teams webhook": re.compile(r"hooks\.slack\.com/services/T[A-Z0-9]{6,}/B[A-Z0-9]{6,}/[A-Za-z0-9]{12,}"),
    "JWT": re.compile(r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}"),
    "Checkgate key with real entropy": re.compile(r"\b(sk_live|cg_pat|pat)_(?=[A-Za-z0-9]*\d)(?=[A-Za-z0-9]*[A-Z])[A-Za-z0-9]{24,}\b"),
    "local home path": re.compile(r"(/Users/[a-z][a-z0-9_.-]+/|/home/[a-z][a-z0-9_.-]+/)(?!\.\.\.)"),
}

TEXT = (".html", ".js", ".css", ".json", ".svg", ".txt", ".xml")


def main():
    if not os.path.isdir(DIST):
        print(f"error: {DIST} does not exist; build the docs first", file=sys.stderr)
        return 2
    problems = []
    files = 0
    for dirpath, _, names in os.walk(DIST):
        for name in names:
            path = os.path.join(dirpath, name)
            rel = os.path.relpath(path, DIST).replace(os.sep, "/")
            files += 1
            if not any(p.match(rel) for p in ALLOWED):
                problems.append(f"{rel}: not an allowed output path")
                continue
            if not rel.endswith(TEXT):
                continue
            with open(path, encoding="utf-8", errors="replace") as f:
                text = f.read()
            for rule, pat in {**INTERNAL, **SECRETS}.items():
                n = len(pat.findall(text))
                if n:
                    problems.append(f"{rel}: {rule} ({n}x)")
    if files == 0:
        problems.append("the output directory is empty")
    if problems:
        print(f"error: {len(problems)} publication problem(s) in {os.path.relpath(DIST, ROOT)}:", file=sys.stderr)
        for p in sorted(problems):
            print(f"  {p}", file=sys.stderr)
        print("Internal records belong in internal_docs/ (gitignored); see docs/.vitepress/config.ts srcExclude.",
              file=sys.stderr)
        return 1
    print(f"ok: {files} files in the docs output, all allowed; no internal references or credentials")
    return 0


if __name__ == "__main__":
    sys.exit(main())
