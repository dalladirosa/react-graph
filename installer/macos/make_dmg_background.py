"""Draws the background of the macOS install window (the .dmg): the brand header, a title, a curved
arrow from the app icon to the Applications folder, and the icon's route as a footer. Writes
dmg-background.png (1x) and dmg-background@2x.png next to this script; bundle.sh merges them.
Finder draws the two icons and their labels on top, at the positions in dmg_settings.py.

    python installer/macos/make_dmg_background.py   (needs Pillow; run on macOS for the SF font)
"""

import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).resolve().parent
ICON = HERE.parent.parent / "assets" / "icon.png"
FONT = "/System/Library/Fonts/SFNS.ttf"
W, H = 640, 400  # window content size; keep in sync with dmg_settings.py
APP_X, APPS_X, ICON_Y = 170, 470, 225  # icon centres; keep in sync with dmg_settings.py
INK = (28, 25, 38)
MUTED = (112, 108, 124)
PINK = (255, 31, 117)
KINDS = [(59, 130, 246), (139, 92, 246), (16, 185, 129)]  # same dots as the app icon


def font(size: float, weight: str) -> ImageFont.FreeTypeFont:
    f = ImageFont.truetype(FONT, round(size))
    f.set_variation_by_name(weight)
    return f


def bezier(p0, p1, p2, p3, n=64):
    pts = []
    for i in range(n + 1):
        t, u = i / n, 1 - i / n
        pts.append(tuple(u**3 * a + 3 * u * u * t * b + 3 * u * t * t * c + t**3 * d
                         for a, b, c, d in zip(p0, p1, p2, p3)))
    return pts


def centered(d: ImageDraw.ImageDraw, y: float, text: str, f, fill, k: int):
    x = (W * k - d.textlength(text, font=f)) / 2
    d.text((x, y * k), text, font=f, fill=fill)


def render(k: int) -> Image.Image:
    img = Image.new("RGB", (W * k, H * k))
    top, bottom = (251, 249, 247), (247, 240, 250)
    for y in range(H * k):
        t = y / (H * k)
        img.paste(tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)), (0, y, W * k, y + 1))
    d = ImageDraw.Draw(img)

    # soft wave behind the icons
    wave = bezier((0, 170), (200, 140), (420, 200), (W, 160))
    d.polygon([(x * k, y * k) for x, y in wave] + [(W * k, H * k), (0, H * k)], fill=(250, 238, 246))

    # header: icon + name
    icon = Image.open(ICON).convert("RGBA").resize((26 * k, 26 * k), Image.LANCZOS)
    img.paste(icon, (22 * k, 18 * k), icon)
    d.text((56 * k, 21 * k), "Rendermap", font=font(16 * k, "Semibold"), fill=INK)

    centered(d, 58, "Drag Rendermap into Applications.", font(26 * k, "Bold"), INK, k)
    centered(d, 98, "Once copied, open it from Applications.", font(14 * k, "Medium"), MUTED, k)

    # arrow from the app to Applications
    arc = bezier((APP_X + 78, ICON_Y + 10), (APP_X + 120, ICON_Y - 40),
                 (APPS_X - 120, ICON_Y - 40), (APPS_X - 80, ICON_Y))
    d.line([(x * k, y * k) for x, y in arc], fill=(60, 56, 72), width=3 * k, joint="curve")
    (px, py), (tx, ty) = arc[-6], arc[-1]
    a = math.atan2(ty - py, tx - px)  # arrowhead follows the curve's end direction
    wings = [(tx - 14 * math.cos(a + s), ty - 14 * math.sin(a + s)) for s in (0.5, -0.5)]
    d.line([(wings[0][0] * k, wings[0][1] * k), (tx * k, ty * k), (wings[1][0] * k, wings[1][1] * k)],
           fill=(60, 56, 72), width=3 * k, joint="curve")

    # footer: the icon's route, component -> hook -> function
    route = bezier((250, 365), (290, 325), (350, 385), (390, 345))
    d.line([(x * k, y * k) for x, y in route], fill=PINK, width=4 * k, joint="curve")
    for (x, y), c in zip([route[0], route[len(route) // 2], route[-1]], KINDS):
        r = 8 * k
        d.ellipse((x * k - r, y * k - r, x * k + r, y * k + r), fill=c, outline=(255, 255, 255), width=2 * k)
    return img


if __name__ == "__main__":
    render(1).save(HERE / "dmg-background.png")
    render(2).save(HERE / "dmg-background@2x.png")
    print("wrote dmg-background.png and dmg-background@2x.png")
