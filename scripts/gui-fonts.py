#!/usr/bin/env python3
"""Cut koan-gui's Geist Mono faces from the variable font the site ships.

GPUI matches a font by choosing among a family's faces, so a variable font
gives it the default instance alone. The theme's type roles use weights 200
(display), 300 (titles) and 400 (everything else); each is instanced from
site/public/geist-mono.woff2, the file the Mac app bundles, into
crates/koan-gui/assets/fonts as a static TrueType face. The outputs are
checked in, so a build needs no Python.

Usage: uvx --from 'fonttools[woff]' python scripts/gui-fonts.py
"""

from pathlib import Path

from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "site/public/geist-mono.woff2"
OUT = ROOT / "crates/koan-gui/assets/fonts"
WEIGHTS = {200: "ExtraLight", 300: "Light", 400: "Regular"}

for weight, style in WEIGHTS.items():
    font = TTFont(SOURCE)
    face = instancer.instantiateVariableFont(font, {"wght": weight}, updateFontNames=True)
    face.flavor = None
    # One typographic family, so each text system finds the three faces as
    # weights of "Geist Mono" rather than as three families.
    names = face["name"]
    names.removeNames(nameID=16)
    names.removeNames(nameID=17)
    names.setName("Geist Mono", 16, 3, 1, 0x409)
    names.setName(style, 17, 3, 1, 0x409)
    face.save(OUT / f"GeistMono-{style}.ttf")
    print(f"GeistMono-{style}.ttf")
