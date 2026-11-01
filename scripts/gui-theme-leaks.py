#!/usr/bin/env python3
"""koan-gui's leaks past its design system (docs/design/koan-kit.md).

Pages build from koan-kit's components and take sizes, colours and type from
its roles. This counts, in each koan-gui source file, what chooses a look by
hand: a size or colour written as a number, GPUI's fixed-size styles, a
toolbar button or page title built from parts, `gpui::` named directly, and
each name taken from `koan_kit::raw`, GPUI reached around the components.
A raw use with no comment on the line above saying why koan-kit cannot cover
it fails the check outright.

A ratchet: crates/koan-gui/theme-leaks.toml holds each file's counts as the
pages are now. The check fails when a count grows or a file not listed has
any; a file that has moved onto the components drops its entry. `--update`
rewrites the baseline, and refuses to raise a count.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "crates/koan-gui/src"
BASELINE = ROOT / "crates/koan-gui/theme-leaks.toml"

KINDS = {
    # A size, a colour or a face chosen by hand.
    "number": re.compile(r"(?<![.\w])(px|rems|rgb|rgba|hsla|font)\("),
    # GPUI's fixed-size styles: tailwind's scale, not the theme's.
    "fixed": re.compile(
        r"\.(p|px|py|pt|pb|pl|pr|m|mx|my|mt|mb|ml|mr|gap|gap_x|gap_y|w|h|size|min_w|min_h|max_w|max_h|top|left|right|bottom|inset)_[1-9]\d*(p\d+)?\("
        r"|\.text_(xs|sm|base|lg|xl|\dxl)\(|\.rounded_(sm|md|lg|xl|full)\(|\.(text_size|line_height|font_family|font_weight)\("
    ),
    # Containers built from parts: a page's title, a toolbar's button.
    "container": re.compile(r"\b(TypeRole|Type)::Title\b|ButtonKind::Toolbar\b"),
    # GPUI named directly rather than through koan-kit.
    "gpui": re.compile(r"\bgpui(_kit)?::"),
}

# GPUI reached through koan-kit's escape hatch, counted by the names taken:
# each is an exception, kept only with a comment saying why koan-kit cannot
# cover it, and the count only falls as pages move onto components.
RAW = re.compile(r"\bkoan_kit::raw::(?:\w+::)*(\{[^}]*\}|\w+)")


def raw_names(text: str) -> int:
    total = 0
    for m in RAW.finditer(text):
        taken = m.group(1)
        total += len([n for n in taken.strip("{}").split(",") if n.strip()]) if taken.startswith("{") else 1
    return total


def unreasoned() -> list[str]:
    """Raw uses with no comment on the line above, or on their own line."""
    missing = []
    for path in sorted(SRC.rglob("*.rs")):
        lines = path.read_text().splitlines()
        for i, line in enumerate(lines):
            code = line.split("//", 1)[0]
            if "koan_kit::raw" not in code or "//" in line:
                continue
            if i == 0 or not lines[i - 1].lstrip().startswith("//"):
                missing.append(f"{path.relative_to(SRC)}:{i + 1}")
    return missing


def counts() -> dict[str, dict[str, int]]:
    found: dict[str, dict[str, int]] = {}
    for path in sorted(SRC.rglob("*.rs")):
        lines = [
            line
            for line in path.read_text().splitlines()
            if not line.lstrip().startswith("//")
        ]
        text = "\n".join(lines)
        per = {kind: len(rx.findall(text)) for kind, rx in KINDS.items()}
        per["raw"] = raw_names(text)
        per = {k: v for k, v in per.items() if v}
        if per:
            found[str(path.relative_to(SRC))] = per
    return found


def read_baseline() -> dict[str, dict[str, int]]:
    baseline: dict[str, dict[str, int]] = {}
    if not BASELINE.exists():
        return baseline
    section = None
    for line in BASELINE.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("["):
            section = line.strip('[]"')
            baseline[section] = {}
        elif section is not None:
            key, value = (part.strip() for part in line.split("=", 1))
            baseline[section][key] = int(value)
    return baseline


def write_baseline(found: dict[str, dict[str, int]]) -> None:
    out = [
        "# What each koan-gui file still chooses by hand, counted by",
        "# scripts/gui-theme-leaks.py. Counts only fall: a file moved onto",
        "# koan-kit's components drops its entry. See docs/design/koan-kit.md.",
        "",
    ]
    for path, per in found.items():
        out.append(f'["{path}"]')
        out.extend(f"{kind} = {n}" for kind, n in per.items())
        out.append("")
    BASELINE.write_text("\n".join(out))


def main() -> int:
    missing = unreasoned()
    if missing:
        print("koan_kit::raw used without a comment above it saying why koan-kit cannot cover it:")
        print("  " + "\n  ".join(missing))
        return 1
    found = counts()
    baseline = read_baseline()
    grown = [
        f"{path}: {kind} {n} (allowed {baseline.get(path, {}).get(kind, 0)})"
        for path, per in found.items()
        for kind, n in per.items()
        if n > baseline.get(path, {}).get(kind, 0)
    ]
    if "--update" in sys.argv:
        if grown and baseline:
            print("refusing to raise the baseline:\n  " + "\n  ".join(grown))
            return 1
        write_baseline(found)
        print(f"baseline: {sum(sum(p.values()) for p in found.values())} leaks in {len(found)} files")
        return 0
    if grown:
        print("koan-gui chooses a look by hand where the design system should:")
        print("  " + "\n  ".join(grown))
        print("Build it from koan-kit's components (docs/design/koan-kit.md).")
        return 1
    shrunk = [p for p, per in baseline.items() if found.get(p, {}) != per]
    if shrunk:
        print("fewer leaks than the baseline in " + ", ".join(shrunk) + ": run `just gui-theme-leaks --update`.")
        return 1
    print(f"koan-gui: {sum(sum(p.values()) for p in found.values())} leaks left in {len(found)} files, none new")
    return 0


if __name__ == "__main__":
    sys.exit(main())
