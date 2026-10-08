#!/usr/bin/env python3
"""Compile the kōan icon set into Swift.

Reads every SVG in apps/macos/Resources/Icons and writes
apps/macos/Sources/Koan/Support/KoanGlyphs.swift: one entry per glyph, its
outline as absolute M/L/C/Z commands on the 24-unit grid, and the SF Symbol
names it stands in for (the root element's `data-sf`).

An element with a `fill` other than `none` belongs to the glyph's fill layer;
every other element is a stroke. Stroke widths in the sources are ignored: the
width is set at draw time from the size, by `KoanGlyph.strokeWidth`.

With --check, writes nothing and fails if the Swift is stale or an `Icon` in
Support/Icons.swift has no glyph.

Usage: scripts/icons.py [--check]
"""

import math
import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCES = ROOT / "apps/macos/Resources/Icons"
OUTPUT = ROOT / "apps/macos/Sources/Koan/Support/KoanGlyphs.swift"
ICONS = ROOT / "apps/macos/Sources/Koan/Support/Icons.swift"
GRID = 24

NUMBER = re.compile(r"[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?")
TOKEN = re.compile(r"[MmLlHhVvCcSsQqTtAaZz]|[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?")


def fmt(value):
    text = f"{round(value, 3):.3f}".rstrip("0").rstrip(".")
    return "0" if text in ("-0", "") else text


def arc_to_cubics(x1, y1, rx, ry, angle, large, sweep, x2, y2):
    """SVG elliptical arc as cubic Béziers (SVG 1.1 appendix F.6)."""
    if (x1, y1) == (x2, y2):
        return []
    if rx == 0 or ry == 0:
        return [(x1, y1, x2, y2, x2, y2)]
    rx, ry = abs(rx), abs(ry)
    phi = math.radians(angle)
    cos, sin = math.cos(phi), math.sin(phi)
    dx, dy = (x1 - x2) / 2, (y1 - y2) / 2
    px, py = cos * dx + sin * dy, -sin * dx + cos * dy
    scale = px * px / (rx * rx) + py * py / (ry * ry)
    if scale > 1:
        rx, ry = rx * math.sqrt(scale), ry * math.sqrt(scale)
    num = rx * rx * ry * ry - rx * rx * py * py - ry * ry * px * px
    den = rx * rx * py * py + ry * ry * px * px
    root = math.sqrt(max(0, num / den)) * (-1 if large == sweep else 1)
    cxp, cyp = root * rx * py / ry, -root * ry * px / rx
    cx = cos * cxp - sin * cyp + (x1 + x2) / 2
    cy = sin * cxp + cos * cyp + (y1 + y2) / 2

    def angle_of(ux, uy, vx, vy):
        a = math.atan2(ux * vy - uy * vx, ux * vx + uy * vy)
        return a

    start = angle_of(1, 0, (px - cxp) / rx, (py - cyp) / ry)
    delta = angle_of((px - cxp) / rx, (py - cyp) / ry, (-px - cxp) / rx, (-py - cyp) / ry)
    if not sweep and delta > 0:
        delta -= 2 * math.pi
    elif sweep and delta < 0:
        delta += 2 * math.pi
    segments = max(1, math.ceil(abs(delta) / (math.pi / 2) - 1e-9))
    step = delta / segments
    k = 4 / 3 * math.tan(step / 4)
    curves = []
    for i in range(segments):
        a1, a2 = start + i * step, start + (i + 1) * step
        e1 = (math.cos(a1) - k * math.sin(a1), math.sin(a1) + k * math.cos(a1))
        e2 = (math.cos(a2) + k * math.sin(a2), math.sin(a2) - k * math.cos(a2))
        e3 = (math.cos(a2), math.sin(a2))
        points = []
        for ex, ey in (e1, e2, e3):
            x, y = ex * rx, ey * ry
            points += [cos * x - sin * y + cx, sin * x + cos * y + cy]
        curves.append(tuple(points))
    return curves


def parse_path(d):
    """A path's data as absolute M/L/C/Z commands."""
    tokens = TOKEN.findall(d)
    out = []
    i = 0
    command = None
    x = y = sx = sy = 0.0
    last_control = None
    last_quad = None

    def take(n):
        nonlocal i
        values = [float(t) for t in tokens[i : i + n]]
        if len(values) != n or any(re.fullmatch(r"[A-Za-z]", t) for t in tokens[i : i + n]):
            raise ValueError(f"bad path data near {' '.join(tokens[i:i + n])!r} in {d!r}")
        i += n
        return values

    while i < len(tokens):
        if re.fullmatch(r"[A-Za-z]", tokens[i]):
            command = tokens[i]
            i += 1
        elif command is None:
            raise ValueError(f"path data must start with a command: {d!r}")
        rel = command.islower()
        c = command.upper()
        ox, oy = (x, y) if rel else (0.0, 0.0)
        control = quad = None
        if c == "Z":
            out.append(("Z",))
            x, y = sx, sy
        elif c == "M":
            nx, ny = take(2)
            x, y = ox + nx, oy + ny
            sx, sy = x, y
            out.append(("M", x, y))
            command = "l" if rel else "L"
        elif c == "L":
            nx, ny = take(2)
            x, y = ox + nx, oy + ny
            out.append(("L", x, y))
        elif c == "H":
            (nx,) = take(1)
            x = ox + nx
            out.append(("L", x, y))
        elif c == "V":
            (ny,) = take(1)
            y = oy + ny
            out.append(("L", x, y))
        elif c in ("C", "S"):
            if c == "C":
                x1, y1, x2, y2, ex, ey = take(6)
                x1, y1 = ox + x1, oy + y1
            else:
                x2, y2, ex, ey = take(4)
                x1, y1 = (2 * x - last_control[0], 2 * y - last_control[1]) if last_control else (x, y)
            x2, y2, ex, ey = ox + x2, oy + y2, ox + ex, oy + ey
            out.append(("C", x1, y1, x2, y2, ex, ey))
            control = (x2, y2)
            x, y = ex, ey
        elif c in ("Q", "T"):
            if c == "Q":
                qx, qy, ex, ey = take(4)
                qx, qy = ox + qx, oy + qy
            else:
                ex, ey = take(2)
                qx, qy = (2 * x - last_quad[0], 2 * y - last_quad[1]) if last_quad else (x, y)
            ex, ey = ox + ex, oy + ey
            out.append(("C", x + 2 / 3 * (qx - x), y + 2 / 3 * (qy - y), ex + 2 / 3 * (qx - ex), ey + 2 / 3 * (qy - ey), ex, ey))
            quad = (qx, qy)
            x, y = ex, ey
        elif c == "A":
            rx, ry, rot, large, sweep, ex, ey = take(7)
            ex, ey = ox + ex, oy + ey
            for curve in arc_to_cubics(x, y, rx, ry, rot, int(large), int(sweep), ex, ey):
                out.append(("C", *curve))
            x, y = ex, ey
        else:
            raise ValueError(f"unsupported path command {command!r}")
        last_control, last_quad = control, quad
    return out


def numbers(element, *names):
    return [float(element.get(name, "0")) for name in names]


def element_commands(element):
    tag = element.tag.split("}")[-1]
    if tag == "path":
        return parse_path(element.get("d", ""))
    if tag == "line":
        x1, y1, x2, y2 = numbers(element, "x1", "y1", "x2", "y2")
        return [("M", x1, y1), ("L", x2, y2)]
    if tag in ("polyline", "polygon"):
        values = [float(v) for v in NUMBER.findall(element.get("points", ""))]
        points = list(zip(values[::2], values[1::2]))
        commands = [("M", *points[0])] + [("L", *p) for p in points[1:]]
        return commands + [("Z",)] if tag == "polygon" else commands
    if tag == "rect":
        x, y, w, h = numbers(element, "x", "y", "width", "height")
        if element.get("rx") or element.get("ry"):
            raise ValueError("rounded rects are not part of the set: corners are square")
        return [("M", x, y), ("L", x + w, y), ("L", x + w, y + h), ("L", x, y + h), ("Z",)]
    if tag in ("circle", "ellipse"):
        cx, cy = numbers(element, "cx", "cy")
        if tag == "circle":
            rx = ry = float(element.get("r", "0"))
        else:
            rx, ry = numbers(element, "rx", "ry")
        return parse_path(f"M{cx - rx} {cy} A{rx} {ry} 0 1 1 {cx + rx} {cy} A{rx} {ry} 0 1 1 {cx - rx} {cy} Z")
    return None


def serialise(commands):
    parts = []
    for command in commands:
        parts.append(command[0] + " ".join(fmt(v) for v in command[1:]))
    return "".join(parts)


def load(path):
    root = ET.parse(path).getroot()
    if root.get("viewBox", "").split() != ["0", "0", str(GRID), str(GRID)]:
        raise ValueError(f"{path.name}: viewBox must be 0 0 {GRID} {GRID}")
    symbols = root.get("data-sf", "").split()
    strokes, fills = [], []
    for element in root.iter():
        if element is root:
            continue
        commands = element_commands(element)
        if commands is None:
            continue
        fill = element.get("fill", root.get("fill", "none"))
        (fills if fill != "none" else strokes).append(serialise(commands))
    if not strokes and not fills:
        raise ValueError(f"{path.name}: no shapes")
    return path.stem, symbols, "".join(strokes), "".join(fills)


def render(glyphs):
    lines = [
        "// Generated by `just icons` from apps/macos/Resources/Icons. Edit the SVGs, not this file.",
        "",
        "extension KoanGlyph {",
        "    /// Every glyph in the set, by name.",
        "    static let all: [String: KoanGlyph] = [",
    ]
    for name, symbols, strokes, fills in glyphs:
        args = [f'name: "{name}"', f'symbols: [{", ".join(chr(34) + s + chr(34) for s in symbols)}]', f'strokes: "{strokes}"']
        if fills:
            args.append(f'fills: "{fills}"')
        lines.append(f'        "{name}": KoanGlyph({", ".join(args)}),')
    lines += ["    ]", "", "    /// The glyph standing in for each SF Symbol.", "    static let bySymbol: [String: String] = ["]
    seen = {}
    for name, symbols, _, _ in glyphs:
        for symbol in symbols:
            if symbol in seen:
                raise ValueError(f"{symbol} is claimed by both {seen[symbol]} and {name}")
            seen[symbol] = name
            lines.append(f'        "{symbol}": "{name}",')
    lines += ["    ]", "}", ""]
    return "\n".join(lines)


def main():
    glyphs = [load(path) for path in sorted(SOURCES.glob("*.svg"))]
    text = render(glyphs)
    covered = {symbol for _, symbols, _, _ in glyphs for symbol in symbols}
    missing = sorted(set(re.findall(r'static let \w+ = "([^"]+)"', ICONS.read_text())) - covered)
    if "--check" in sys.argv:
        if OUTPUT.read_text() != text:
            sys.exit(f"{OUTPUT.relative_to(ROOT)} is stale: run `just icons`")
        if missing:
            sys.exit(f"no glyph for {', '.join(missing)}: add an SVG whose data-sf names it")
        return
    for symbol in missing:
        print(f"warning: no glyph for {symbol}", file=sys.stderr)
    OUTPUT.write_text(text)
    print(f"{len(glyphs)} glyphs → {OUTPUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
