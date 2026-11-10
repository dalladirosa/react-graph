"""Draws the Rendermap app icon: a folded paper map on the pink-to-violet brand tile, with a
pink route joining a component, a hook and a function across the folds. Writes icon.png
(512px) and app.ico (16-256px) next to this script.

    python assets/make_icon.py
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

HERE = Path(__file__).resolve().parent
S = 1024  # design size; every size is drawn at 4x and scaled down
PINK = (255, 31, 117)
VIOLET = (124, 92, 255)
WHITE = (255, 255, 255)
FOLD = (233, 229, 248)  # the panel that faces away from the light
KINDS = [(59, 130, 246), (139, 92, 246), (16, 185, 129)]  # component, hook, function


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
    """The icon at `px` square. Small sizes fill the tile, enlarge the map and thicken the route."""
    small = px <= 32
    k = px * 4 / S  # supersampled scale
    w = round(S * k)
    f = lambda v: v * k  # noqa: E731

    # tile: diagonal pink -> violet gradient, rounded
    inset = 0 if small else 36
    radius = 180 if small else 200
    grad = Image.new("RGBA", (w, w))
    gp = grad.load()
    for y in range(w):
        for x in range(0, w):
            t = min(1.0, max(0.0, (x + y) / (2 * (w - 1))))
            gp[x, y] = tuple(round(a + (b - a) * t) for a, b in zip(PINK, VIOLET)) + (255,)
    img = Image.new("RGBA", (w, w), (0, 0, 0, 0))
    mask = Image.new("L", (w, w), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [f(inset), f(inset), f(S - inset) - 1, f(S - inset) - 1], radius=f(radius), fill=255
    )
    img.paste(grad, (0, 0), mask)

    # folded map: three panels whose top and bottom edges zig-zag at the folds
    x0, x3 = (130, 894) if small else (190, 834)
    xs = [x0 + (x3 - x0) * i / 3 for i in range(4)]
    hgt = 600 if small else 500
    zig = 46 if small else 52
    top = 512 - hgt / 2
    tops = [top + zig, top, top + zig, top]
    panels = [[(xs[i], tops[i]), (xs[i + 1], tops[i + 1]), (xs[i + 1], tops[i + 1] + hgt), (xs[i], tops[i] + hgt)] for i in range(3)]

    # soft shadow under the map
    sh = Image.new("L", (w, w), 0)
    sd = ImageDraw.Draw(sh)
    for p in panels:
        sd.polygon([(f(x), f(y + 26)) for x, y in p], fill=120)
    sh = sh.filter(ImageFilter.GaussianBlur(f(28)))
    img.paste(Image.new("RGBA", (w, w), (40, 10, 70, 255)), (0, 0), ImageChops_mul(sh, mask))

    d = ImageDraw.Draw(img)
    for i, p in enumerate(panels):
        d.polygon([(f(x), f(y)) for x, y in p], fill=FOLD if i == 1 else WHITE)

    # route across the folds
    if small:
        nodes = [(xs[0] + 105, 660), (512, 370), (xs[3] - 105, 620)]
    else:
        nodes = [(xs[0] + 92, 650), (512, 405), (xs[3] - 92, 600)]
    stroke = 76 if small else 34
    rr = f(stroke) / 2
    for a, b in zip(nodes, nodes[1:]):
        m = (b[0] - a[0]) * 0.6
        for x, y in bezier(a, (a[0] + m, a[1]), (b[0] - m, b[1]), b, n=500):
            d.ellipse([f(x) - rr, f(y) - rr, f(x) + rr, f(y) + rr], fill=PINK)

    # nodes: kind-coloured dots, the middle one (the hook) ringed in white
    nr = 78 if small else 46
    for i, ((x, y), kind) in enumerate(zip(nodes, KINDS)):
        if i == 1 and not small:
            ring = 14
            d.ellipse([f(x - nr - ring), f(y - nr - ring), f(x + nr + ring), f(y + nr + ring)], fill=WHITE)
        d.ellipse([f(x - nr), f(y - nr), f(x + nr), f(y + nr)], fill=kind)

    return img.resize((px, px), Image.LANCZOS)


def ImageChops_mul(a, b):  # noqa: N802
    from PIL import ImageChops

    return ImageChops.multiply(a, b)


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
