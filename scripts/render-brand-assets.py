#!/usr/bin/env python3
"""Render Checkgate's brand graphics from assets/branding/tokens.json.

    python3 scripts/render-brand-assets.py           # write the SVGs
    python3 scripts/render-brand-assets.py --check   # fail if a committed SVG is stale

Writes, for the light and dark themes:
  assets/banner-{light,dark}.svg                  README hero (1600 x 600)
  assets/architecture/overview-{light,dark}.svg   how sync and evaluation differ
  assets/social/og-image.svg                      social preview source (1200 x 630)

The social preview PNG (docs/public/og-image.png: the docs site's og:image,
and the image to upload as the GitHub repository's social preview) is a raster
of og-image.svg at 1200 x 630; re-export it after changing the SVG (any SVG
rasterizer; the committed one was rendered in Chrome with the Inter font that
VitePress bundles).

Deterministic: the same tokens always give byte-identical files. Text uses
system fonts (a README <img> cannot load web fonts) and states no measured
numbers: performance figures live only in the generated benchmark graphics
(bench/public/scripts/render_svg.py), which are rendered from the raw results.
"""

import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
with open(os.path.join(ROOT, "assets", "branding", "tokens.json"), encoding="utf-8") as _f:
    TOKENS = json.load(_f)
SANS = TOKENS["font"]["sans"].replace("'", "&apos;")
MONO = TOKENS["font"]["mono"].replace("'", "&apos;")
INDIGO = TOKENS["indigo"]

HEADLINE = ("Ship with confidence.", "Roll out with control.")
DESCRIPTION = "Open-source, self-hosted feature management with local evaluation and real-time synchronization."
TAGLINE = "Feature flags without the round-trip."
CHIPS = ("Open source", "Self-hosted", "Rust-powered")


def esc(s):
    return str(s).replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace('"', "&quot;")


def text(x, y, s, size, fill, weight=400, anchor="start", family=None, extra=""):
    fam = f' font-family="{family}"' if family else ""
    return (f'<text x="{x}" y="{y}" font-size="{size}" font-weight="{weight}" fill="{fill}" '
            f'text-anchor="{anchor}"{fam}{extra}>{esc(s)}</text>')


def mark(x, y, size, stroke):
    """The Checkgate gate mark (48 x 48 artwork) at (x, y), scaled to `size`."""
    k = size / 48
    return (f'<g transform="translate({x} {y}) scale({k:.4f})" fill="none" stroke="{stroke}" '
            f'stroke-width="6" stroke-linecap="round">'
            f'<path d="M33.4 13.6 A14 14 0 1 0 33.4 34.4"/><path d="M23 24 H39"/></g>')


def chip(x, y, label, t, size=20, pad=18, char_w=0.56):
    w = round(len(label) * size * char_w + 2 * pad)
    h = round(size * 1.9)
    return w, (f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{h // 2}" fill="{t["surface"]}" '
               f'stroke="{t["border"]}"/>'
               + text(x + w / 2, y + h / 2 + size * 0.35, label, size, t["ink2"], 500, "middle"))


def network(cx, cy, t, scale=1.0):
    """Decorative motif: one server node pushing to SDK nodes that each evaluate locally."""
    nodes = [(-250, -170), (-40, -215), (175, -160), (250, 20), (150, 190), (-90, 200), (-265, 60)]
    out = [f'<g opacity="0.95">']
    # faint rings around the server
    for r, o in ((150, 0.55), (235, 0.35)):
        out.append(f'<circle cx="{cx}" cy="{cy}" r="{r * scale:.0f}" fill="none" stroke="{t["border"]}" '
                   f'stroke-width="1.5" opacity="{o}"/>')
    for dx, dy in nodes:
        x, y = cx + dx * scale, cy + dy * scale
        mx, my = cx + dx * scale * 0.5 + dy * scale * 0.12, cy + dy * scale * 0.5 - dx * scale * 0.12
        out.append(f'<path d="M{cx} {cy} Q{mx:.0f} {my:.0f} {x:.0f} {y:.0f}" fill="none" stroke="{t["accent"]}" '
                   f'stroke-width="2" stroke-dasharray="2 8" stroke-linecap="round" opacity="0.7"/>')
    for i, (dx, dy) in enumerate(nodes):
        x, y = cx + dx * scale, cy + dy * scale
        s = 46 * scale
        out.append(f'<rect x="{x - s / 2:.0f}" y="{y - s / 2:.0f}" width="{s:.0f}" height="{s:.0f}" rx="{12 * scale:.0f}" '
                   f'fill="{t["surface"]}" stroke="{t["border"]}" stroke-width="1.5"/>')
        # local evaluation: a small filled dot inside each SDK node
        out.append(f'<circle cx="{x:.0f}" cy="{y:.0f}" r="{7 * scale:.0f}" fill="{t["eval"]}"/>')
    s = 120 * scale
    out.append(f'<rect x="{cx - s / 2:.0f}" y="{cy - s / 2:.0f}" width="{s:.0f}" height="{s:.0f}" rx="{28 * scale:.0f}" '
               f'fill="{INDIGO["600"]}"/>')
    out.append(mark(cx - 34 * scale, cy - 34 * scale, 68 * scale, "#FFFFFF"))
    out.append("</g>")
    return out


def dot_grid(x0, y0, x1, y1, step, color):
    """A light dot grid as one path (small file, no pattern/url references)."""
    d = []
    y = y0
    while y <= y1:
        x = x0
        while x <= x1:
            d.append(f"M{x} {y}h0.01")
            x += step
        y += step
    return (f'<path d="{" ".join(d)}" stroke="{color}" stroke-width="2.6" stroke-linecap="round" fill="none"/>')


def banner(theme):
    t = TOKENS[theme]
    W, H = 1600, 600
    title = f"Checkgate — {HEADLINE[0]} {HEADLINE[1]}"
    desc = f"{DESCRIPTION} {' · '.join(CHIPS)}."
    glow = INDIGO["500"]
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" '
        f'role="img" aria-labelledby="t d" font-family="{SANS}">',
        f'<title id="t">{esc(title)}</title>',
        f'<desc id="d">{esc(desc)}</desc>',
        "<defs>",
        f'<radialGradient id="glow" cx="0.78" cy="0.42" r="0.55">'
        f'<stop offset="0" stop-color="{glow}" stop-opacity="{0.22 if theme == "dark" else 0.10}"/>'
        f'<stop offset="1" stop-color="{glow}" stop-opacity="0"/></radialGradient>',
        f'<linearGradient id="fade" x1="0" x2="1"><stop offset="0" stop-color="{t["bg"]}" stop-opacity="1"/>'
        f'<stop offset="0.35" stop-color="{t["bg"]}" stop-opacity="0"/></linearGradient>',
        "</defs>",
        f'<rect width="{W}" height="{H}" fill="{t["bg"]}"/>',
        dot_grid(860, 40, 1580, 560, 40, t["grid"]),
        f'<rect x="840" width="{W - 840}" height="{H}" fill="url(#fade)"/>',
        f'<rect width="{W}" height="{H}" fill="url(#glow)"/>',
    ]
    out += network(1255, 300, t, 1.0)
    # wordmark
    out.append(mark(112, 92, 56, t["accent"]))
    out.append(text(182, 134, "Checkgate", 40, t["ink"], 700, extra=' letter-spacing="-0.5"'))
    # headline
    out.append(text(112, 262, HEADLINE[0], 68, t["ink"], 700, extra=' letter-spacing="-1.5"'))
    out.append(text(112, 344, HEADLINE[1], 68, t["accent"], 700, extra=' letter-spacing="-1.5"'))
    out.append(text(114, 404, "Open-source, self-hosted feature management with local", 26, t["ink2"]))
    out.append(text(114, 440, "evaluation and real-time synchronization.", 26, t["ink2"]))
    x = 112
    for label in CHIPS:
        w, svg = chip(x, 488, label, t)
        out.append(svg)
        x += w + 12
    out.append("</svg>")
    return "\n".join(out) + "\n"


def arrow_marker(mid, color):
    return (f'<marker id="{mid}" viewBox="0 0 10 10" refX="8.5" refY="5" markerWidth="7" markerHeight="7" '
            f'orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="{color}"/></marker>')


def card(x, y, w, h, t, fill=None, stroke=None, rx=16, extra=""):
    return (f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" fill="{fill or t["surface"]}" '
            f'stroke="{stroke or t["border"]}" stroke-width="1.5"{extra}/>')


def step(x, y, n, color, t):
    return (f'<circle cx="{x}" cy="{y}" r="13" fill="{color}"/>'
            + text(x, y + 5, str(n), 14, t["bg"], 700, "middle"))


def overview(theme):
    t = TOKENS[theme]
    W, H = 1200, 600
    acc, ev = t["accent"], t["eval"]
    title = "How Checkgate works: configuration syncs to SDKs; flags evaluate inside your process"
    desc = ("1. A flag changes in the dashboard or through the API. 2. The Checkgate server commits it to "
            "PostgreSQL, fans it out to every server instance through Redis, and pushes it over a Server-Sent "
            "Events stream to each connected SDK; SDKs reconnect with backoff and poll the snapshot endpoint if a "
            "stream cannot be established. 3. The SDK keeps the flags in a local store inside your process. "
            "4. Your code calls isEnabled(), which the SDK's Rust engine answers from that store with no network "
            "request. The server never evaluates your application's flag checks; SDKs that report impressions send them "
            "in background batches.")
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" '
        f'role="img" aria-labelledby="t d" font-family="{SANS}">',
        f'<title id="t">{esc(title)}</title>',
        f'<desc id="d">{esc(desc)}</desc>',
        "<defs>", arrow_marker("a-sync", acc), arrow_marker("a-eval", ev), arrow_marker("a-muted", t["muted"]), "</defs>",
        f'<rect x="0.5" y="0.5" width="{W - 1}" height="{H - 1}" rx="20" fill="{t["bg"]}" stroke="{t["border"]}"/>',
        text(40, 58, "How Checkgate works", 24, t["ink"], 700),
        text(40, 86, "Configuration syncs in the background. Flag checks are answered inside your process.", 15, t["ink2"]),
    ]
    # Column 1: control plane
    out.append(card(40, 150, 214, 186, t))
    out.append(text(62, 186, "Dashboard / API", 17, t["ink"], 700))
    for i, line in enumerate(("Edit a flag, rule or", "rollout. Automation", "uses tokens: Terraform,", "operator, MCP agents.")):
        out.append(text(62, 214 + i * 22, line, 14, t["ink2"]))
    out.append(step(64, 312, 1, acc, t))
    out.append(text(86, 317, "a flag changes", 13, t["muted"], 600))

    # Column 2: server
    sx, sw = 300, 320
    out.append(card(sx, 120, sw, 246, t, stroke=acc))
    out.append(text(sx + 24, 156, "Checkgate server", 17, t["ink"], 700))
    out.append(text(sx + 24, 178, "Rust · self-hosted", 13, t["muted"]))
    rows = (("PostgreSQL", "flags, revisions, audit"), ("Redis", "fan-out across instances"),
            ("SSE", "one stream per SDK"))
    for i, (name, what) in enumerate(rows):
        y = 196 + i * 50
        out.append(card(sx + 20, y, sw - 40, 40, t, fill=t["surface2"], rx=10))
        out.append(text(sx + 36, y + 25, name, 14, acc if name == "SSE" else t["ink"], 700))
        out.append(text(sx + 130, y + 25, what, 13, t["ink2"]))
    out.append(step(sx + 24, 352, 2, acc, t))
    out.append(text(sx + 46, 357, "commit, then push", 13, t["muted"], 600))

    # Column 3: your process
    px, pw = 780, 380
    out.append(card(px, 110, pw, 400, t, fill=t["bg"], extra=' stroke-dasharray="6 6"'))
    out.append(text(px + 24, 142, "Your service — one process", 15, t["muted"], 600))
    out.append(card(px + 20, 160, pw - 40, 168, t, stroke=ev))
    out.append(text(px + 42, 194, "Checkgate SDK", 17, t["ink"], 700))
    out.append(card(px + 40, 210, 164, 96, t, fill=t["surface2"], rx=10))
    out.append(text(px + 56, 240, "Local flag store", 14, t["ink"], 700))
    for i, line in enumerate(("last known flags;", "kept if the stream", "drops")):
        out.append(text(px + 56, 262 + i * 17, line, 12.5, t["ink2"]))
    out.append(card(px + 220, 210, 120, 96, t, fill=t["surface2"], rx=10))
    out.append(text(px + 234, 240, "Rust engine", 14, t["ink"], 700))
    for i, line in enumerate(("same code in", "every SDK")):
        out.append(text(px + 234, 262 + i * 17, line, 12.5, t["ink2"]))
    out.append(f'<path d="M{px + 204} 258 H{px + 217}" stroke="{t["muted"]}" stroke-width="2" marker-end="url(#a-muted)"/>')
    # app code
    out.append(card(px + 20, 392, pw - 40, 96, t, fill=t["surface2"]))
    out.append(text(px + 42, 424, "Your code", 15, t["ink"], 700))
    out.append(text(px + 42, 452, "isEnabled('new-checkout', user)", 14, ev, 600, family=MONO))
    out.append(text(px + 42, 474, "answered in-process · no network request", 13, t["ink2"]))
    # evaluation arrows (code <-> engine)
    ex = px + 266
    out.append(f'<path d="M{ex} 392 V312" stroke="{ev}" stroke-width="2.5" marker-end="url(#a-eval)"/>')
    out.append(f'<path d="M{ex + 30} 312 V392" stroke="{ev}" stroke-width="2.5" marker-end="url(#a-eval)"/>')
    out.append(step(ex - 28, 352, 4, ev, t))
    out.append(text(ex - 50, 357, "every check", 13, t["muted"], 600, "end"))

    # sync arrows (gap between the server and the process: x 620-780)
    out.append(f'<path d="M254 243 H{sx - 4}" stroke="{acc}" stroke-width="2.5" marker-end="url(#a-sync)"/>')
    out.append(f'<path d="M{sx + sw} 262 C{sx + sw + 70} 262 {px - 40} 236 {px + 16} 236" fill="none" '
               f'stroke="{acc}" stroke-width="2.5" marker-end="url(#a-sync)"/>')
    out.append(step(sx + sw + 22, 206, 3, acc, t))
    out.append(text(sx + sw + 42, 211, "push over SSE", 13, acc, 700))
    out.append(text(sx + sw + 12, 282, "heartbeats ·", 12, t["muted"]))
    out.append(text(sx + sw + 12, 298, "reconnect with backoff", 12, t["muted"]))
    out.append(f'<path d="M{px + 16} 316 C{px - 50} 316 {sx + sw + 60} 340 {sx + sw + 4} 340" fill="none" '
               f'stroke="{t["muted"]}" stroke-width="1.75" stroke-dasharray="5 6" marker-end="url(#a-muted)"/>')
    for i, line in enumerate(("polls the snapshot only", "if a stream cannot be", "established")):
        out.append(text(sx + sw + 12, 370 + i * 16, line, 12, t["muted"]))

    # legend
    ly = 548
    out.append(f'<line x1="40" y1="530" x2="{W - 40}" y2="530" stroke="{t["border"]}"/>')
    out.append(f'<path d="M40 {ly - 5} H76" stroke="{acc}" stroke-width="3" marker-end="url(#a-sync)"/>')
    out.append(text(88, ly, "Sync path — runs when configuration changes", 14, t["ink2"], 600))
    out.append(f'<path d="M520 {ly - 5} H556" stroke="{ev}" stroke-width="3" marker-end="url(#a-eval)"/>')
    out.append(text(568, ly, "Evaluation path — every flag check, inside your process", 14, t["ink2"], 600))
    out.append(text(40, ly + 28, "No request per flag check; impressions, when enabled, are batched in the background. A dropped stream keeps the last known flags.",
                    12.5, t["muted"]))
    out.append("</svg>")
    return "\n".join(o for o in out if o) + "\n"


def og_image():
    t = TOKENS["dark"]
    W, H = 1200, 630
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" '
        f'role="img" aria-labelledby="t d" font-family="{SANS}">',
        f'<title id="t">Checkgate — {esc(HEADLINE[0])} {esc(HEADLINE[1])}</title>',
        f'<desc id="d">{esc(DESCRIPTION)}</desc>',
        "<defs>",
        f'<radialGradient id="glow" cx="0.85" cy="0.35" r="0.6"><stop offset="0" stop-color="{INDIGO["500"]}" '
        f'stop-opacity="0.24"/><stop offset="1" stop-color="{INDIGO["500"]}" stop-opacity="0"/></radialGradient>',
        f'<linearGradient id="fade" x1="0" x2="1"><stop offset="0" stop-color="{t["bg"]}" stop-opacity="1"/>'
        f'<stop offset="0.4" stop-color="{t["bg"]}" stop-opacity="0"/></linearGradient>',
        "</defs>",
        f'<rect width="{W}" height="{H}" fill="{t["bg"]}"/>',
        dot_grid(700, 30, 1180, 600, 40, t["grid"]),
        f'<rect x="680" width="{W - 680}" height="{H}" fill="url(#fade)"/>',
        f'<rect width="{W}" height="{H}" fill="url(#glow)"/>',
    ]
    out += network(965, 330, t, 0.8)
    out.append(mark(72, 72, 52, t["accent"]))
    out.append(text(136, 112, "Checkgate", 38, t["ink"], 700, extra=' letter-spacing="-0.5"'))
    out.append(text(72, 250, HEADLINE[0], 58, t["ink"], 700, extra=' letter-spacing="-1.2"'))
    out.append(text(72, 322, HEADLINE[1], 58, t["accent"], 700, extra=' letter-spacing="-1.2"'))
    out.append(text(74, 384, "Open-source, self-hosted feature management", 24, t["ink2"]))
    out.append(text(74, 416, "with local evaluation and real-time sync.", 24, t["ink2"]))
    x = 72
    for label in CHIPS:
        w, svg = chip(x, 466, label, t, size=19)
        out.append(svg)
        x += w + 12
    out.append(text(72, 584, "github.com/checkgate-sh/checkgate", 19, t["muted"], 500, family=MONO))
    out.append("</svg>")
    return "\n".join(out) + "\n"


def outputs():
    files = {}
    for theme in ("light", "dark"):
        files[f"assets/banner-{theme}.svg"] = banner(theme)
        files[f"assets/architecture/overview-{theme}.svg"] = overview(theme)
    files["assets/social/og-image.svg"] = og_image()
    return files


def main():
    check = "--check" in sys.argv[1:]
    stale = []
    for rel, body in outputs().items():
        path = os.path.join(ROOT, rel)
        if check:
            try:
                with open(path, encoding="utf-8") as f:
                    if f.read() != body:
                        stale.append(rel)
            except OSError:
                stale.append(rel)
            continue
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8", newline="\n") as f:
            f.write(body)
        print(f"wrote {rel} ({len(body.encode())} bytes)")
    if stale:
        print("error: stale brand graphics (run python3 scripts/render-brand-assets.py): " + ", ".join(stale),
              file=sys.stderr)
        return 1
    if check:
        print("ok: brand graphics match assets/branding/tokens.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())
