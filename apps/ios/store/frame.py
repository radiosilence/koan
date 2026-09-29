# /// script
# requires-python = ">=3.12"
# dependencies = ["pillow"]
# ///
"""Frame App Store screenshots: the screen on a blur of its own colours, with a caption.

    uv run apps/ios/store/frame.py captions.toml SRC_DIR OUT_DIR

`captions.toml` maps each screenshot's file stem to its caption, in the order
they should appear. The output keeps the source's pixel size, which is the size
App Store Connect asks for, and numbers the files in that order.
"""

import sys
import tomllib
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

FONT = "/System/Library/Fonts/SFNS.ttf"


def frame(src: Path, caption: str, out: Path):
    shot = Image.open(src).convert("RGB")
    w, h = shot.size

    # The record's own colours, as the ground: the screen blurred past
    # recognition and lifted a little so the caption reads on it.
    ground = shot.resize((w // 8, h // 8)).filter(ImageFilter.GaussianBlur(24)).resize((w, h))
    ground = Image.blend(ground, Image.new("RGB", (w, h), "white"), 0.25)

    # A tablet's screen is nearly square and its type small: less margin,
    # and a caption sized to the shorter band left above it.
    tablet = h / w < 1.6
    scale = 0.84 if tablet else 0.80
    sw, sh = int(w * scale), int(h * scale)
    screen = shot.resize((sw, sh), Image.LANCZOS)
    radius = int(sw * 0.075)
    mask = Image.new("L", (sw, sh), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, sw, sh), radius, fill=255)

    x, y = (w - sw) // 2, h - sh - int(h * 0.035)
    shadow = Image.new("L", (w, h), 0)
    ImageDraw.Draw(shadow).rounded_rectangle((x, y + 18, x + sw, y + sh + 18), radius, fill=90)
    shadow = shadow.filter(ImageFilter.GaussianBlur(40))
    ground.paste(Image.new("RGB", (w, h), (20, 30, 40)), (0, 0), shadow)
    ground.paste(screen, (x, y), mask)

    draw = ImageDraw.Draw(ground)
    size = int(w * (0.045 if tablet else 0.062))
    font = ImageFont.truetype(FONT, size)
    font.set_variation_by_name("Semibold")
    lines = wrap(draw, caption, font, int(w * 0.84))
    line_h = int(size * 1.18)
    top = (y - line_h * len(lines)) // 2
    for i, line in enumerate(lines):
        tw = draw.textlength(line, font=font)
        draw.text(((w - tw) / 2, top + i * line_h), line, font=font, fill=(24, 28, 34))
    ground.save(out, optimize=True)


def wrap(draw, text, font, width):
    """One line if it fits, otherwise two of about equal length: a caption
    that leaves one word on its own line reads as a mistake."""
    if draw.textlength(text, font=font) <= width:
        return [text]
    words = text.split()
    splits = [(" ".join(words[:i]), " ".join(words[i:])) for i in range(1, len(words))]
    return list(min(splits, key=lambda p: max(draw.textlength(p[0], font=font), draw.textlength(p[1], font=font))))


def main():
    captions = tomllib.loads(Path(sys.argv[1]).read_text())
    src, out = Path(sys.argv[2]), Path(sys.argv[3])
    out.mkdir(parents=True, exist_ok=True)
    for i, (stem, caption) in enumerate(captions.items(), 1):
        frame(src / f"{stem}.png", caption, out / f"{i}-{stem}.png")
        print(f"{i}-{stem}: {caption}")


if __name__ == "__main__":
    main()
