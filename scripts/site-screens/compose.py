"""Turn the renderer's PNGs into the website's screenshots.

The full-window pages are encoded as they are. The control and output menus
are popovers, which the renderer draws on their own: they are laid over the
window's lower-right corner here, as they open over the transport, and the
crop is the one the site has always used.

    python3 compose.py <renders> <site/public/screens>
"""
import pathlib
import subprocess
import sys
import tempfile

RAW = pathlib.Path(sys.argv[1]).resolve()
OUT = pathlib.Path(sys.argv[2]).resolve()
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

PAGES = {
    "mac-album": "site-mac-album-dark",
    "mac-artist": "site-mac-artist-dark",
    "mac-favourites": "site-mac-favourites-dark",
    "mac-lyrics": "site-mac-lyrics-dark",
    "mac-search": "site-mac-search-dark",
}

# Name, canvas (CSS px, drawn at 2x), the popover's render, and where the
# window's lower-right corner sits on the canvas. The menu opens above the
# transport, at its own size, its right edge past the window's as a popover
# from a button near that edge is.
MENUS = [
    ("mac-control", (1082, 537), "popover-control-dark", (908, 470)),
    ("mac-output", (1097, 522), "popover-output-dark", (920, 470)),
]


def webp(png, name, quality=75):
    target = OUT / f"{name}.webp"
    subprocess.run(["cwebp", "-quiet", "-q", str(quality), "-alpha_q", "100", "-m", "6", str(png), "-o", str(target)],
                   check=True)
    print(f"{target.name}: {target.stat().st_size // 1024} KB")


def menu(name, canvas, popover, corner):
    w, h = canvas
    right, bottom = corner
    window = RAW / "site-mac-queue-dark.png"
    page = f"""<!doctype html><html><head><style>
html,body{{margin:0;width:{w}px;height:{h}px;overflow:hidden;background:transparent}}
img{{position:absolute;display:block}}
.window{{left:{right - 1200}px;top:{bottom - 750}px;width:1200px;border-radius:12px;
  box-shadow:0 22px 60px rgb(0 0 0 / .45)}}
.menu{{right:{w - right - 90}px;bottom:{h - bottom + 74}px;width:340px;box-shadow:0 16px 40px rgb(0 0 0 / .55);
  outline:1px solid rgb(255 255 255 / .08)}}
</style></head><body>
<img class="window" src="file://{window}"><img class="menu" src="file://{RAW / (popover + '.png')}">
</body></html>"""
    with tempfile.TemporaryDirectory() as tmp:
        html = pathlib.Path(tmp) / "menu.html"
        png = pathlib.Path(tmp) / "menu.png"
        html.write_text(page)
        subprocess.run([CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars", "--force-device-scale-factor=2",
                        f"--window-size={w},{h}", "--default-background-color=00000000", "--allow-file-access-from-files",
                        f"--screenshot={png}", f"file://{html}"], check=True, capture_output=True)
        webp(png, name)


for name, raw in PAGES.items():
    webp(RAW / f"{raw}.png", name)
for args in MENUS:
    menu(*args)
