#!/usr/bin/env python3
"""Render the README performance graphic from summary.json.

    render_svg.py docs/performance/results/v0.1.30/summary.json assets/benchmarks

Writes performance-light.svg and performance-dark.svg (the README picks one
with <picture> + prefers-color-scheme, like the other README graphics).
Deterministic: the same summary.json always gives byte-identical files. Every
number comes from summary["headline"]; a metric missing there is left out of
the graphic, never filled with a placeholder.

No scripts, no external fonts or images: a README <img> blocks both, so text
uses the system UI font stack.
"""

import json
import os
import sys

W, H = 1200, 420
PAD = 40
FONT = "system-ui,-apple-system,'Segoe UI',Roboto,Helvetica,Arial,sans-serif"

THEMES = {
    # GitHub's own surfaces and inks; the accent is Checkgate indigo. Green,
    # red and amber are reserved for flag state and never used here.
    "light": {"bg": "#ffffff", "border": "#d0d7de", "rule": "#d8dee4", "ink": "#1f2328",
              "ink2": "#424a53", "muted": "#59636e", "accent": "#4f46e5"},
    "dark": {"bg": "#0d1117", "border": "#30363d", "rule": "#30363d", "ink": "#f0f6fc",
             "ink2": "#c9d1d9", "muted": "#9198a1", "accent": "#818cf8"},
}

ORDER = [
    ("evaluation", "Evaluation latency"),
    ("throughput", "Server throughput"),
    ("propagation", "Real-time propagation"),
]


def esc(s):
    return str(s).replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace('"', "&quot;")


def sig3(v):
    """Round to three significant figures (precision the runs support)."""
    if v == 0:
        return 0
    from math import floor, log10
    digits = 2 - int(floor(log10(abs(v))))
    return round(v, digits)


def fmt_value(h):
    v, unit = h["value"], h["unit"]
    if unit == "ns":
        if v >= 1000:
            return f"{sig3(v / 1000):g}", "µs"
        return f"{int(round(v))}", "ns"
    if unit == "ms":
        return (f"{v:.1f}" if v < 10 else f"{int(round(v))}"), "ms"
    if unit == "req/s":
        return f"{int(sig3(v)):,}", "req/s"
    return f"{v:g}", unit


def wrap(text, size, width):
    """Greedy word wrap using an average glyph width of 0.53 em."""
    max_chars = max(10, int(width / (size * 0.53)))
    lines, line = [], ""
    for word in text.split(" "):
        candidate = f"{line} {word}".strip()
        if len(candidate) > max_chars and line:
            lines.append(line)
            line = word
        else:
            line = candidate
    if line:
        lines.append(line)
    return lines


def text(x, y, s, size, fill, weight=400, anchor="start", extra=""):
    return (f'<text x="{x}" y="{y}" font-size="{size}" font-weight="{weight}" fill="{fill}" '
            f'text-anchor="{anchor}"{extra}>{esc(s)}</text>')


def render(summary, theme):
    t = THEMES[theme]
    head = summary["headline"]
    tiles = [(k, label, head[k]) for k, label in ORDER if k in head]
    env = summary["environment"]
    commit = summary["commit"][:7]
    desc_parts = []
    for _, label, h in tiles:
        num, unit = fmt_value(h)
        desc_parts.append(f"{label}: {num} {unit} ({h['scenario']}; {h['context']}).")
    desc = (" ".join(desc_parts) + f" Measured {summary['date']} on {env['host']}; "
            f"server limited to {env['server_limits']}; commit {commit}.")
    title = f"Checkgate v{summary['version']} performance benchmarks"

    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" '
        f'role="img" aria-labelledby="t d" font-family="{FONT}">',
        f'<title id="t">{esc(title)}</title>',
        f'<desc id="d">{esc(desc)}</desc>',
        f'<rect x="0.5" y="0.5" width="{W - 1}" height="{H - 1}" rx="16" fill="{t["bg"]}" stroke="{t["border"]}"/>',
        text(PAD, 64, "Checkgate performance", 24, t["ink"], 600),
        text(W - PAD, 64, f"v{summary['version']} · reproducible benchmarks · {summary['date']}",
             14, t["muted"], 400, "end"),
        f'<line x1="{PAD}" y1="88" x2="{W - PAD}" y2="88" stroke="{t["rule"]}"/>',
    ]
    n = max(1, len(tiles))
    col = (W - 2 * PAD) / n
    for i, (_, label, h) in enumerate(tiles):
        x = round(PAD + i * col + (0 if i == 0 else 32))
        inner = col - (32 if i == 0 else 64)
        if i > 0:
            lx = round(PAD + i * col)
            out.append(f'<line x1="{lx}" y1="112" x2="{lx}" y2="318" stroke="{t["rule"]}"/>')
        num, unit = fmt_value(h)
        out.append(f'<rect x="{x}" y="116" width="28" height="4" rx="2" fill="{t["accent"]}"/>')
        out.append(text(x, 150, label, 16, t["ink2"], 600))
        out.append(
            f'<text x="{x}" y="222" fill="{t["ink"]}" font-weight="600">'
            f'<tspan font-size="64">{esc(num)}</tspan>'
            f'<tspan font-size="24" fill="{t["ink2"]}" dx="8">{esc(unit)}</tspan></text>')
        y = 258
        for line in wrap(h["scenario"], 15, inner)[:2]:
            out.append(text(x, y, line, 15, t["ink2"]))
            y += 22
        for line in wrap(h["context"], 13.5, inner)[:2]:
            out.append(text(x, y + 2, line, 13.5, t["muted"]))
            y += 20
    out.append(f'<line x1="{PAD}" y1="338" x2="{W - PAD}" y2="338" stroke="{t["rule"]}"/>')
    out.append(text(PAD, 366, f"{env['short_host']} · {env['docker']} · server limited to "
                               f"{env['server_limits']} · PostgreSQL 16 + Redis 7 in containers", 13, t["muted"]))
    out.append(text(PAD, 390, f"{summary['repetitions']} runs per benchmark, commit {commit}. Method, every "
                               f"run and the raw data: docs/performance/benchmarks.md", 13, t["muted"]))
    out.append("</svg>")
    return "\n".join(out) + "\n"


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    with open(sys.argv[1], encoding="utf-8") as f:
        summary = json.load(f)
    if not summary.get("headline"):
        sys.exit("summary.json has no measured headline metrics; not rendering a graphic")
    os.makedirs(sys.argv[2], exist_ok=True)
    for theme in THEMES:
        path = os.path.join(sys.argv[2], f"performance-{theme}.svg")
        with open(path, "w", encoding="utf-8", newline="\n") as f:
            f.write(render(summary, theme))


if __name__ == "__main__":
    main()
