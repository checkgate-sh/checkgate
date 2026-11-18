#!/usr/bin/env python3
"""Render the README benchmark graphics.

    render_svg.py docs/performance/results/v0.1.31/summary.json assets/benchmarks

Writes performance-{light,dark}.svg ("Architecture & Performance", from
summary.json) and, when cost-summary.json sits next to summary.json,
cost-{light,dark}.svg ("Infrastructure Cost & Savings"). The README picks a
theme with <picture> + prefers-color-scheme, like its other graphics.
Deterministic: the same inputs always give byte-identical files. Every number
comes from the summaries; a metric missing there is left out, never filled
with a placeholder.

No scripts, no external fonts or images: a README <img> blocks both, so text
uses the system UI font stack.
"""

import json
import os
import sys

W = 1200
PAD = 40
FONT = "system-ui,-apple-system,'Segoe UI',Roboto,Helvetica,Arial,sans-serif"
NOTE = "Measured under documented workloads and hardware. Results may vary by deployment."

THEMES = {
    # GitHub's own surfaces and inks; the accent is Checkgate indigo. Green,
    # red and amber are reserved for flag state and never used here.
    "light": {"bg": "#ffffff", "border": "#d0d7de", "rule": "#d8dee4", "ink": "#1f2328",
              "ink2": "#424a53", "muted": "#59636e", "accent": "#4f46e5", "bar2": "#c7c9f2"},
    "dark": {"bg": "#0d1117", "border": "#30363d", "rule": "#30363d", "ink": "#f0f6fc",
             "ink2": "#c9d1d9", "muted": "#9198a1", "accent": "#818cf8", "bar2": "#3b3f73"},
}

ORDER = [
    ("evaluation", "SDK evaluation", "lower"),
    ("throughput", "Server throughput", "higher"),
    ("efficiency", "CPU and memory", "lower"),
    ("propagation", "Update propagation", "lower"),
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
    if unit in ("ns", "µs"):
        ns = v if unit == "ns" else v * 1000
        if ns >= 1_000_000:
            return f"{sig3(ns / 1_000_000):g}", "ms"
        if ns >= 10_000:
            return f"{sig3(ns / 1000):g}", "µs"
        if ns >= 1000:
            return f"{ns / 1000:.1f}", "µs"
        return f"{int(round(ns))}", "ns"
    if unit == "ms":
        return (f"{v:.1f}" if v < 10 else f"{int(round(v))}"), "ms"
    if unit == "req/s":
        return f"{int(sig3(v)):,}", "req/s"
    return f"{v:g}", unit


def change(h, better):
    """'v0.1.30: X · 6.9× faster' from the measured baseline, if there is one."""
    base = h.get("baseline_value")
    if not base or not h["value"]:
        return None
    num, unit = fmt_value({**h, "value": base})
    factor = (base / h["value"]) if better == "lower" else (h["value"] / base)
    if factor >= 1.05:
        word = "faster" if h["key"] in ("evaluation", "propagation") else "better"
        if h["key"] == "throughput":
            word = "more"
        if h["key"] == "efficiency":
            word = "less CPU"
            return f"{num} {unit} before · {sig3(1 - 1 / factor) * 100:g} % {word}"
        return f"{num} {unit} before · {sig3(factor):g}× {word}"
    if factor <= 0.95:
        return f"{num} {unit} before · {sig3(1 / factor):g}× worse"
    return f"{num} {unit} before · unchanged"


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


def frame(t, h, title, desc):
    return [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {h}" width="{W}" height="{h}" '
        f'role="img" aria-labelledby="t d" font-family="{FONT}">',
        f'<title id="t">{esc(title)}</title>',
        f'<desc id="d">{esc(desc)}</desc>',
        f'<rect x="0.5" y="0.5" width="{W - 1}" height="{h - 1}" rx="16" fill="{t["bg"]}" stroke="{t["border"]}"/>',
    ]


def render(summary, theme):
    t = THEMES[theme]
    H = 520
    head = summary["headline"]
    base_version = (summary.get("baseline") or {}).get("version")
    tiles = [(k, label, better, {**head[k], "key": k}) for k, label, better in ORDER if k in head]
    env = summary["environment"]
    commit = summary["commit"][:7]
    desc_parts = []
    for _, label, better, h in tiles:
        num, unit = fmt_value(h)
        c = change(h, better)
        desc_parts.append(f"{label}: {num} {unit} ({h['scenario']}; {h['context']}"
                          + (f"; v{base_version}: {c}" if c else "") + ").")
    desc = (" ".join(desc_parts) + f" Measured {summary['date']} on {env['host']}; "
            f"server limited to {env['server_limits']}; commit {commit}. {NOTE}")
    title = f"Checkgate v{summary['version']}: architecture and performance"
    out = frame(t, H, title, desc)
    out += [
        text(PAD, 60, "Architecture & Performance", 24, t["ink"], 600),
        text(W - PAD, 60, f"v{summary['version']} · measured {summary['date']}", 14, t["muted"], 400, "end"),
        text(PAD, 86, "Local evaluation. Real-time updates. Lightweight infrastructure.", 15, t["ink2"]),
        f'<line x1="{PAD}" y1="104" x2="{W - PAD}" y2="104" stroke="{t["rule"]}"/>',
    ]
    n = max(1, len(tiles))
    col = (W - 2 * PAD) / n
    for i, (_, label, better, h) in enumerate(tiles):
        x = round(PAD + i * col + (0 if i == 0 else 24))
        inner = col - (24 if i == 0 else 48)
        if i > 0:
            lx = round(PAD + i * col)
            out.append(f'<line x1="{lx}" y1="124" x2="{lx}" y2="358" stroke="{t["rule"]}"/>')
        num, unit = fmt_value(h)
        out.append(f'<rect x="{x}" y="128" width="28" height="4" rx="2" fill="{t["accent"]}"/>')
        out.append(text(x, 158, label, 15, t["ink2"], 600))
        out.append(
            f'<text x="{x}" y="216" fill="{t["ink"]}" font-weight="600">'
            f'<tspan font-size="48">{esc(num)}</tspan>'
            f'<tspan font-size="20" fill="{t["ink2"]}" dx="6">{esc(unit)}</tspan></text>')
        y = 244
        c = change(h, better)
        if c:
            out.append(text(x, y, f"v{base_version}: {c}", 13.5, t["accent"], 600))
            y += 24
        for line in wrap(h["scenario"], 13.5, inner)[:3]:
            out.append(text(x, y, line, 13.5, t["ink2"]))
            y += 19
        for line in wrap(h["context"], 12.5, inner)[:3]:
            out.append(text(x, y + 2, line, 12.5, t["muted"]))
            y += 18
    out.append(f'<line x1="{PAD}" y1="378" x2="{W - PAD}" y2="378" stroke="{t["rule"]}"/>')
    arch = [
        ("Local evaluation", "SDKs evaluate flags in-process: no request per flag decision."),
        ("Server-sent events", "One persistent connection per client carries flag changes."),
        ("Rust server", "Measured here inside a 1 vCPU / 512 MiB container."),
    ]
    acol = (W - 2 * PAD) / 3
    for i, (name, line) in enumerate(arch):
        ax = round(PAD + i * acol)
        out.append(text(ax, 404, name, 13.5, t["ink"], 600))
        out.append(text(ax, 424, line, 12.5, t["ink2"]))
    out.append(f'<line x1="{PAD}" y1="446" x2="{W - PAD}" y2="446" stroke="{t["rule"]}"/>')
    comp = f" Earlier version measured the same way, runs alternated (v{base_version})." if base_version else ""
    out.append(text(PAD, 472, f"{env['short_host']} · {env['docker']} · server limited to {env['server_limits']} · "
                              f"PostgreSQL 16 + Redis 7 in containers · {runs_label(summary)}, commit {commit}.",
                    12.5, t["muted"]))
    out.append(text(PAD, 494, f"{NOTE}{comp} Method and raw data: docs/performance/benchmarks.md", 12.5, t["muted"]))
    out.append("</svg>")
    return "\n".join(out) + "\n"


def runs_label(summary):
    """'3 runs' or '3 runs (propagation 6)' from the run log."""
    runs = summary.get("runs") or {}
    n = summary["repetitions"]
    extra = [f"{k} {v['ok']}" for k, v in runs.items()
             if not k.startswith("cost-") and v.get("ok") != n]
    return f"{n} runs" + (f" ({', '.join(extra)})" if extra else "")


def fmt_int(v):
    return f"{int(round(v)):,}"


def fmt_bytes(b):
    if b >= 1e9:
        return f"{sig3(b / 1e9):g} GB"
    if b >= 1e6:
        return f"{sig3(b / 1e6):g} MB"
    if b >= 1e3:
        return f"{sig3(b / 1e3):g} KB"
    return f"{int(b)} B"


def fmt_ms(v):
    if v is None:
        return "—"
    return f"{sig3(v / 1000):g} s" if v >= 1000 else f"{int(round(v))} ms"


def cost_focus(cost):
    """The client count compared in the SSE-vs-polling panel: 1,000 if measured, else the largest SSE count."""
    sse = [p for p in cost["measured"]["sse"] if p.get("runs")]
    counts = sorted(p["clients"] for p in sse)
    if not counts:
        return None
    return 1000 if 1000 in counts else counts[-1]


def render_cost(cost, theme):
    t = THEMES[theme]
    H = 574
    sse = cost["measured"]["sse"]
    polls = cost["measured"]["poll"]
    traffic = cost["calculated"]["monthly_traffic"]
    monthly = cost["projected"]["monthly"]
    a = cost["calculated"]["assumptions"]
    focus = cost_focus(cost)
    title = f"Checkgate v{cost['version']}: infrastructure cost and savings"
    desc_bits = []
    out_rows = []
    out = []
    out += [
        text(PAD, 60, "Infrastructure Cost & Savings", 24, t["ink"], 600),
        text(W - PAD, 60, f"v{cost['version']} · prices accessed {cost['pricing']['accessed']}", 14, t["muted"], 400, "end"),
        text(PAD, 86, "Understand the infrastructure required to run your own feature flags.", 15, t["ink2"]),
        f'<line x1="{PAD}" y1="104" x2="{W - PAD}" y2="104" stroke="{t["rule"]}"/>',
    ]
    # Panel A: connected clients on one server
    ax, aw = PAD, 360
    out.append(f'<rect x="{ax}" y="124" width="28" height="4" rx="2" fill="{t["accent"]}"/>')
    out.append(text(ax, 154, "Streaming clients on one server", 15, t["ink2"], 600))
    out.append(text(ax, 174, "measured · 1 vCPU / 512 MiB limit · idle, heartbeats only", 12, t["muted"]))
    y = 204
    for hdr, hx in (("Clients", ax), ("CPU", ax + 120), ("RSS", ax + 200), ("Heartbeats", ax + 270)):
        out.append(text(hx, y, hdr, 12, t["muted"], 600))
    y += 24
    for p in sorted(sse, key=lambda p: p["clients"]):
        out.append(text(ax, y, fmt_int(p["clients"]), 14, t["ink"], 600))
        if p.get("runs") and p["sustained"]:
            cpu = p["idle"]["server_cpu_pct"]
            rss = p["idle"]["server_rss_mib"]
            hb = p["idle"]["heartbeat_bytes_per_client_per_hour"]
            out.append(text(ax + 120, y, f"{sig3(cpu):g} %", 14, t["ink"]))
            out.append(text(ax + 200, y, f"{int(round(rss))} MiB", 14, t["ink"]))
            out.append(text(ax + 270, y, f"{fmt_bytes(hb)}/h", 14, t["ink"]))
            desc_bits.append(f"{fmt_int(p['clients'])} streaming clients: {sig3(cpu):g} % of one CPU idle, "
                             f"{int(round(rss))} MiB RSS, {fmt_bytes(hb)} of heartbeats per client per hour")
        else:
            out.append(text(ax + 120, y, "not sustained at this limit", 13, t["muted"]))
            desc_bits.append(f"{fmt_int(p['clients'])} streaming clients: not sustained at this limit")
        y += 26
    out.append(text(ax, y + 4, "CPU: % of one core. Per-client heartbeat bytes, not counting TCP/TLS.", 11.5, t["muted"]))

    # Panel B: SSE vs polling at `focus` clients
    bx, bw = PAD + 400, 380
    out.append(f'<line x1="{bx - 20}" y1="124" x2="{bx - 20}" y2="420" stroke="{t["rule"]}"/>')
    out.append(f'<rect x="{bx}" y="124" width="28" height="4" rx="2" fill="{t["accent"]}"/>')
    out.append(text(bx, 154, f"{fmt_int(focus)} clients: streaming vs polling" if focus else "Streaming vs polling", 15, t["ink2"], 600))
    out.append(text(bx, 174, "traffic per month (calculated) · freshness (measured)", 12, t["muted"]))
    rows = []
    s_tr = next((x for x in traffic if x["mode"] == "sse" and x["clients"] == focus), None)
    s_pr = next((p for p in sse if p["clients"] == focus and p.get("runs")), None)
    if s_tr and s_pr:
        rows.append(("SSE", s_tr["gb_per_month"], s_tr["requests_per_month"],
                     s_pr["updates"]["delivery_p50_ms"], s_pr["sustained"]))
    for p in sorted((p for p in polls if p["clients"] == focus), key=lambda p: -p["interval_s"]):
        tr = next((x for x in traffic if x["mode"] == "poll" and x["clients"] == focus and x["interval_s"] == p["interval_s"]), None)
        rows.append((f"Poll {p['interval_s']} s", tr["gb_per_month"] if tr else None,
                     tr["requests_per_month"] if tr else None, p.get("staleness_p50_ms"), p["sustained"]))
    gmax = max([r[1] for r in rows if r[1]] or [1])
    y = 204
    for hdr, hx in (("", bx), ("Traffic / month", bx + 92), ("Freshness p50", bx + 300)):
        out.append(text(hx, y, hdr, 12, t["muted"], 600))
    y += 26
    for name, gb, reqs, fresh, ok in rows:
        out.append(text(bx, y, name, 14, t["ink"], 600))
        if gb is not None:
            wbar = max(2, round(120 * gb / gmax))
            out.append(f'<rect x="{bx + 92}" y="{y - 12}" width="{wbar}" height="14" rx="3" '
                       f'fill="{t["accent"] if name == "SSE" else t["bar2"]}"/>')
            label = f"{sig3(gb):g} GB" if gb >= 1 else f"{int(round(gb * 1000))} MB"
            out.append(text(bx + 92 + wbar + 8, y, label, 13, t["ink"]))
        out.append(text(bx + 300, y, fmt_ms(fresh) + ("" if ok else " *"), 13, t["ink"]))
        desc_bits.append(f"{name} at {fmt_int(focus)} clients: {('%g GB' % sig3(gb)) if gb is not None else 'n/a'} a month, "
                         f"{fmt_int(reqs) if reqs is not None else 'n/a'} requests a month, freshness p50 {fmt_ms(fresh)}"
                         + ("" if ok else " (not sustained)"))
        y += 28
    out.append(text(bx, y + 2, f"Assumes {a['flag_changes_per_day']} flag changes/day, "
                               f"{a['stream_reconnects_per_client_per_day']} reconnects/client/day.", 11.5, t["muted"]))
    out.append(text(bx, y + 20, "Each poll returns the whole flag set (no conditional requests).", 11.5, t["muted"]))
    if any(not r[4] for r in rows):
        out.append(text(bx, y + 38, "* not sustained by one server at this limit", 11.5, t["muted"]))

    # Panel C: projected monthly cost
    cx = PAD + 820
    out.append(f'<line x1="{cx - 20}" y1="124" x2="{cx - 20}" y2="420" stroke="{t["rule"]}"/>')
    out.append(f'<rect x="{cx}" y="124" width="28" height="4" rx="2" fill="{t["accent"]}"/>')
    out.append(text(cx, 154, "Projected monthly cost", 15, t["ink2"], 600))
    out.append(text(cx, 174, "AWS us-east-1 on-demand list prices", 12, t["muted"]))
    y = 210
    priced = sorted((m for m in monthly if m["mode"] == "sse" and m.get("total_usd") is not None),
                    key=lambda m: m["clients"])
    # Consecutive client counts with the same projected total share one row.
    groups = []
    for m in priced:
        if groups and groups[-1][-1]["total_usd"] == m["total_usd"]:
            groups[-1].append(m)
        else:
            groups.append([m])
    for g in groups:
        lo, hi, usd = g[0]["clients"], g[-1]["clients"], g[0]["total_usd"]
        label = f"{fmt_int(lo)} clients" if lo == hi else f"{fmt_int(lo)}–{fmt_int(hi)} clients"
        out.append(text(cx, y, f"Streaming, {label}", 14, t["ink2"]))
        out.append(text(W - PAD, y, f"${usd:,.2f}", 18, t["ink"], 600, "end"))
        desc_bits.append(f"projected ${usd:,.2f} a month for {label} streaming")
        y += 22
        if len(g) > 1:
            out.append(text(cx, y, "same infrastructure at each of these counts", 11.5, t["muted"]))
            y += 24
        else:
            y += 6
    for m in sorted((m for m in monthly if m["mode"] == "sse" and m.get("total_usd") is None), key=lambda m: m["clients"]):
        out.append(text(cx, y, f"{fmt_int(m['clients'])} clients", 14, t["ink2"]))
        out.append(text(W - PAD, y, "not priced", 13, t["muted"], 400, "end"))
        y += 28
    focus_polls = sorted((m for m in monthly if m["mode"] == "poll" and m["clients"] == focus), key=lambda m: -m["interval_s"])
    if focus_polls:
        out.append(text(cx, y + 2, f"{fmt_int(focus)} clients, polling instead:", 12, t["muted"]))
        y += 24
        for m in focus_polls:
            out.append(text(cx, y, f"every {m['interval_s']} s", 14, t["ink2"]))
            if m.get("total_usd") is not None:
                out.append(text(W - PAD, y, f"${m['total_usd']:,.2f}", 16, t["ink"], 600, "end"))
                desc_bits.append(f"projected ${m['total_usd']:,.2f} a month for {fmt_int(focus)} clients polling every {m['interval_s']} s")
            else:
                out.append(text(W - PAD, y, "not sustained", 13, t["muted"], 400, "end"))
            y += 24
    out.append(text(cx, y + 6, "1 server + PostgreSQL + Redis + storage", 12, t["muted"]))
    out.append(text(cx, y + 24, "+ egress. A projection, not a quote.", 12, t["muted"]))

    out.append(f'<line x1="{PAD}" y1="440" x2="{W - PAD}" y2="440" stroke="{t["rule"]}"/>')
    foot = [
        "Measured: CPU, memory, bytes and freshness of one server at 1 vCPU / 512 MiB, with PostgreSQL and Redis in containers.",
        "Calculated: monthly traffic from measured bytes per heartbeat, change and reconnect at the stated rates. Projected: priced with",
        "dated third-party list prices. Polling is Checkgate's own snapshot endpoint on the same infrastructure, not another product.",
        NOTE,
    ]
    for i, line in enumerate(foot):
        out.append(text(PAD, 466 + i * 20, line, 12, t["muted"]))
    out.append(text(PAD, 554, "Method, assumptions, prices and raw data: docs/performance/cost-efficiency.md", 12, t["muted"]))
    desc = "; ".join(desc_bits) + f". Prices accessed {cost['pricing']['accessed']}, {cost['pricing']['region']}. {NOTE}"
    return "\n".join(frame(t, H, title, desc) + out + ["</svg>"]) + "\n"


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
    cost_path = os.path.join(os.path.dirname(sys.argv[1]), "cost-summary.json")
    if os.path.exists(cost_path):
        with open(cost_path, encoding="utf-8") as f:
            cost = json.load(f)
        for theme in THEMES:
            path = os.path.join(sys.argv[2], f"cost-{theme}.svg")
            with open(path, "w", encoding="utf-8", newline="\n") as f:
                f.write(render_cost(cost, theme))


if __name__ == "__main__":
    main()
