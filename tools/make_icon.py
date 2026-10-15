"""Draws the RideCore app icon (1024 px PNG): a dusk sky, a low sun, a
climb's silhouette with the road snaking up it, and the RIDE/CORE logo.

Usage: python3 tools/make_icon.py <out.png>
"""
import math
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

S = 1024
ROOT = Path(__file__).resolve().parent.parent
FONT = ROOT / "assets/fonts/BarlowCondensed-ExtraBold.ttf"
ORANGE = (255, 122, 26)


def lerp(a, b, t):
    return tuple(int(x + (y - x) * t) for x, y in zip(a, b))


def main(out):
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    # Dusk sky, as on the start screen.
    sky = Image.new("RGBA", (S, S))
    px = sky.load()
    stops = [(0.0, (8, 10, 22)), (0.55, (38, 30, 66)), (0.85, (170, 80, 60)), (1.0, (190, 95, 60))]
    for y in range(S):
        t = y / S
        for (t0, c0), (t1, c1) in zip(stops, stops[1:]):
            if t0 <= t <= t1:
                c = lerp(c0, c1, (t - t0) / (t1 - t0))
                break
        for x in range(S):
            px[x, y] = (*c, 255)
    # A low sun with a glow.
    glow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    g = ImageDraw.Draw(glow)
    sun = (700, 720)
    g.ellipse([sun[0] - 260, sun[1] - 260, sun[0] + 260, sun[1] + 260], fill=(255, 170, 90, 120))
    glow = glow.filter(ImageFilter.GaussianBlur(90))
    sky = Image.alpha_composite(sky, glow)
    d = ImageDraw.Draw(sky)
    d.ellipse([sun[0] - 95, sun[1] - 95, sun[0] + 95, sun[1] + 95], fill=(255, 205, 140, 255))

    # Two ranges of hills and the climb in front.
    def ridge(base, amp, seed, color):
        pts = [(0, S)]
        for k in range(0, S + 1, 8):
            u = k / S * 6.0 + seed
            h = (math.sin(u) * 0.5 + math.sin(u * 2.3 + 1.7) * 0.3 + math.sin(u * 5.1) * 0.12) * amp
            pts.append((k, base - h))
        pts.append((S, S))
        d.polygon(pts, fill=color)

    ridge(760, 60, 0.0, (52, 40, 78, 255))
    ridge(820, 45, 3.0, (30, 26, 52, 255))
    climb = [(0, S)]
    for k in range(0, S + 1, 8):
        x = k / S
        climb.append((k, 990 - 230 * math.exp(-((x - 0.38) / 0.26) ** 2) - 30 * x))
    climb.append((S, S))
    d.polygon(climb, fill=(14, 14, 28, 255))
    # The road up it, in the accent color.
    road = [(k, y - 6) for k, y in climb[1:-1]]
    d.line(road, fill=ORANGE, width=14, joint="curve")

    # The logo.
    font = ImageFont.truetype(str(FONT), 300)
    for text, y, color in [("RIDE", 95, (236, 239, 245)), ("CORE", 340, ORANGE)]:
        w = d.textlength(text, font=font)
        d.text(((S - w) / 2, y), text, font=font, fill=color)

    # macOS app icon shape: a rounded square with a margin.
    inset = 100
    size = S - 2 * inset
    tile = sky.resize((size, size), Image.LANCZOS)
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, size - 1, size - 1], radius=185, fill=255)
    shadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle(
        [inset, inset + 12, inset + size, inset + size + 12], radius=185, fill=(0, 0, 0, 110)
    )
    img = Image.alpha_composite(img, shadow.filter(ImageFilter.GaussianBlur(14)))
    img.paste(tile, (inset, inset), mask)
    img.save(out)


if __name__ == "__main__":
    main(sys.argv[1])
