"""Draws the React Graph app icon: a root card branching into two child cards (the way the
app lays out a component tree), on a dark tile. Writes icon.png (512px) and app.ico
(16-256px) next to this script.

    python assets/make_icon.py
"""

from pathlib import Path

from PIL import Image, ImageChops, ImageDraw

HERE = Path(__file__).resolve().parent
S = 1024  # design size; every size is drawn at 4x and scaled down
PINK = (255, 0, 113)
WHITE = (255, 255, 255)
LINE = (213, 217, 225)
KINDS = [(59, 130, 246), (168, 85, 247), (16, 185, 129)]  # component, hook, function


def bezier(p0, p1, p2, p3, n=96):
    out = []
    for i in range(n + 1):
        t = i / n
        u = 1 - t
        out.append(
            (
                u**3 * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t**3 * p3[0],
                u**3 * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t**3 * p3[1],
            )
        )
    return out


def draw(px: int) -> Image.Image:
    """The icon at `px` square. Small sizes drop the text lines and thicken the strokes."""
    small = px <= 32
    k = px * 4 / S  # supersampled scale
    w = round(S * k)
    img = Image.new("RGBA", (w, w), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    f = lambda v: v * k  # noqa: E731

    # tile: vertical gradient, rounded
    inset = 0 if small else 36
    radius = 200 if not small else 180
    grad = Image.new("RGBA", (w, w))
    gd = ImageDraw.Draw(grad)
    top, bot = (44, 41, 70), (21, 20, 33)
    for y in range(w):
        t = y / (w - 1)
        gd.line([(0, y), (w, y)], fill=tuple(round(a + (b - a) * t) for a, b in zip(top, bot)) + (255,))
    mask = Image.new("L", (w, w), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [f(inset), f(inset), f(S - inset) - 1, f(S - inset) - 1], radius=f(radius), fill=255
    )
    img.paste(grad, (0, 0), mask)

    # cards: (x0, y0, x1, y1, kind)
    cw, ch = (330, 230) if small else (280, 180)
    root = (120, 512 - ch / 2, 150 + cw, 512 + ch / 2, 0)
    gap = 250 if small else 205
    kids = [
        (904 - cw, 512 - gap - ch / 2, 904, 512 - gap + ch / 2, 1),
        (904 - cw, 512 + gap - ch / 2, 904, 512 + gap + ch / 2, 2),
    ]
    if small:
        root = (70, 512 - ch / 2, 70 + cw, 512 + ch / 2, 0)
        kids = [(954 - cw, y0, 954, y1, kk) for _, y0, _, y1, kk in kids]

    # edges first, so the cards sit on top of them
    stroke = 64 if small else 34
    a = (root[2], 512)
    for c in kids:
        b = (c[0], (c[1] + c[3]) / 2)
        m = (b[0] - a[0]) / 2
        # round brush stamped along the curve: smooth at any width (PIL's wide polylines jag)
        rr = f(stroke) / 2
        for x, y in bezier(a, (a[0] + m * 1.2, a[1]), (b[0] - m * 1.2, b[1]), b, n=600):
            d.ellipse([f(x) - rr, f(y) - rr, f(x) + rr, f(y) + rr], fill=PINK)

    r = 30 if small else 36
    strip = 56 if small else 40
    for x0, y0, x1, y1, kind in [root, *kids]:
        box = [f(x0), f(y0), f(x1), f(y1)]
        d.rounded_rectangle(box, radius=f(r), fill=WHITE)
        # coloured strip along the top, clipped to the card's rounded corners
        sm = Image.new("L", (w, w), 0)
        ImageDraw.Draw(sm).rounded_rectangle(box, radius=f(r), fill=255)
        band = Image.new("L", (w, w), 0)
        ImageDraw.Draw(band).rectangle([f(x0), f(y0), f(x1), f(y0 + strip)], fill=255)
        img.paste(Image.new("RGBA", (w, w), KINDS[kind] + (255,)), (0, 0), ImageChops.multiply(sm, band))
        if not small:
            # two "text" lines
            lx = x0 + 34
            for i, frac in enumerate((0.72, 0.46)):
                ly = y0 + strip + 34 + i * 46
                d.rounded_rectangle(
                    [f(lx), f(ly), f(lx + (x1 - x0 - 68) * frac), f(ly + 22)], radius=f(11), fill=LINE
                )

    # connection handles
    hr = 40 if small else 30
    handles = [a] + [(c[0], (c[1] + c[3]) / 2) for c in kids]
    for x, y in handles:
        d.ellipse([f(x - hr), f(y - hr), f(x + hr), f(y + hr)], fill=PINK)
        if not small:
            ir = hr * 0.42
            d.ellipse([f(x - ir), f(y - ir), f(x + ir), f(y + ir)], fill=WHITE)

    return img.resize((px, px), Image.LANCZOS)


def main():
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    frames = {s: draw(s) for s in sizes}
    draw(512).save(HERE / "icon.png")
    frames[256].save(
        HERE / "app.ico",
        format="ICO",
        sizes=[(s, s) for s in sizes],
        append_images=[frames[s] for s in sizes if s != 256],
    )
    print("wrote icon.png, app.ico")


if __name__ == "__main__":
    main()
