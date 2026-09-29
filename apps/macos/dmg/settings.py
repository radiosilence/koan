# dmgbuild settings for Koan.dmg. `just macos-dmg` passes the paths in with -D.
#
# dmgbuild writes the window layout into .DS_Store itself rather than scripting
# Finder, so it works on a runner with no logged-in session. The window size and
# icon centres must match those `background.swift` draws around.

import os.path
import unicodedata

app = defines["app"]  # noqa: F821 - supplied by dmgbuild
# HFS+ stores names decomposed; the layout is keyed by the name as stored.
app_name = unicodedata.normalize("NFD", os.path.basename(app))

format = "UDZO"
filesystem = "HFS+"
files = [app]
symlinks = {"Applications": "/Applications"}
icon = defines["volume_icon"]  # noqa: F821

background = defines["background"]  # noqa: F821
window_rect = ((200, 120), (660, 400))
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False

default_view = "icon-view"
icon_size = 128
text_size = 13
arrange_by = None
icon_locations = {
    app_name: (170, 190),
    "Applications": (490, 190),
}
