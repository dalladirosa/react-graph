# dmgbuild settings for the Rendermap install window. bundle.sh passes app= and background= with -D.
# Sizes and positions match make_dmg_background.py.
import os.path

app = defines["app"]  # noqa: F821 (injected by dmgbuild)
format = "UDZO"
files = [app]
symlinks = {"Applications": "/Applications"}
background = defines["background"]  # noqa: F821
window_rect = ((200, 120), (640, 400))
default_view = "icon-view"
show_status_bar = show_tab_view = show_toolbar = show_pathbar = show_sidebar = False
icon_size = 112
text_size = 14
icon_locations = {os.path.basename(app): (170, 225), "Applications": (470, 225)}
