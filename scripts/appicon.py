#!/usr/bin/env -S uv run --with pillow --with numpy --script
"""Render the Sesh app icon: Catppuccin Mocha clouds under a neon "sesh".

    uv run scripts/appicon.py                 # write every variant to build/icons
    uv run scripts/appicon.py --sheet         # ... and a contact sheet next to them
    uv run scripts/appicon.py --install NAME  # install one into the asset catalogue

A variant names a cloud photo from Resources/icon-clouds, a colour ramp and a glow
recipe, joined by double underscores, as printed by a plain run.
"""

import argparse
import json
import pathlib

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont, ImageOps

ROOT = pathlib.Path(__file__).resolve().parent.parent
CLOUDS = ROOT / "Resources" / "icon-clouds"
FONTS = ROOT / "Resources" / "Fonts"
APPICON = ROOT / "Resources" / "Icons.xcassets" / "AppIcon.appiconset"
OUT = ROOT / "build" / "icons"

SS, FINAL = 2048, 1024

MOCHA = dict(
    crust="#11111b", mantle="#181825", base="#1e1e2e", surface0="#313244",
    surface1="#45475a", surface2="#585b70", text="#cdd6f4", lavender="#b4befe",
    mauve="#cba6f7", blue="#89b4fa", sapphire="#74c7ec", sky="#89dceb",
    teal="#94e2d5", green="#a6e3a1", pink="#f5c2e7", peach="#fab387",
    yellow="#f9e2af", rosewater="#f5e0dc",
)

RAMPS = {
    "night": [(0.0, "crust"), (0.45, "mantle"), (0.75, "surface0"), (1.0, "surface1")],
    "slate": [(0.0, "mantle"), (0.5, "base"), (0.8, "surface1"), (1.0, "surface2")],
    "violet": [(0.0, "crust"), (0.5, "#1c1630"), (0.82, "#342b52"), (1.0, "#4a3f6b")],
}

# glow layers are (blur as a fraction of the canvas, colour, brightness)
GLOWS = {
    "mauve-bloom": ([(0.075, "mauve", 0.80), (0.022, "mauve", 0.60),
                     (0.006, "lavender", 0.18)], "text", "solid"),
    "sky-blue": ([(0.080, "blue", 0.75), (0.024, "sapphire", 0.60),
                  (0.006, "sky", 0.18)], "text", "solid"),
    "pink-blue": ([(0.095, "blue", 0.70), (0.026, "pink", 0.70),
                   (0.006, "pink", 0.20)], "text", "solid"),
    "ember-peach": ([(0.085, "peach", 0.75), (0.024, "yellow", 0.55),
                     (0.006, "yellow", 0.18)], "rosewater", "solid"),
    "tight-teal": ([(0.016, "teal", 0.85), (0.004, "green", 0.25)], "text", "solid"),
    "wide-halo": ([(0.170, "mauve", 0.75), (0.050, "lavender", 0.45),
                   (0.012, "lavender", 0.18)], "text", "solid"),
    "tube-sky": ([(0.060, "sapphire", 0.85), (0.018, "sky", 0.70),
                  (0.003, "sky", 0.45)], "text", "tube"),
    "tube-lavender": ([(0.055, "lavender", 0.85), (0.016, "mauve", 0.70),
                       (0.003, "lavender", 0.45)], "text", "tube"),
    "tube-mauve": ([(0.070, "mauve", 0.90), (0.020, "pink", 0.60),
                    (0.003, "rosewater", 0.45)], "rosewater", "tube"),
}

VARIANTS = [
    ("mammatus", "night", "mauve-bloom"),
    ("mammatus", "night", "ember-peach"),
    ("mammatus", "violet", "pink-blue"),
    ("popcorn", "night", "sky-blue"),
    ("popcorn", "slate", "mauve-bloom"),
    ("popcorn", "night", "tube-sky"),
    ("tower", "night", "wide-halo"),
    ("tower", "night", "tight-teal"),
    ("tower", "violet", "mauve-bloom"),
    ("wisp", "night", "sky-blue"),
    ("wisp", "slate", "tube-lavender"),
    ("ripple", "night", "tight-teal"),
    ("ripple", "night", "tube-sky"),
    ("ripple", "violet", "tube-mauve"),
]


def rgb(name):
    h = MOCHA.get(name, name).lstrip("#")
    return np.array([int(h[i:i + 2], 16) for i in (0, 2, 4)]) / 255


def plate(cloud, ramp, contrast=1.15):
    img = Image.open(CLOUDS / f"{cloud}.jpg").convert("L").resize((SS, SS), Image.LANCZOS)
    g = np.asarray(ImageOps.autocontrast(img, cutoff=1), np.float64) / 255
    rng = np.random.default_rng(7)
    g = np.clip(0.5 + (g + rng.normal(0, 0.014, g.shape) - 0.5) * contrast, 0, 1)
    pos = [p for p, _ in RAMPS[ramp]]
    cols = np.array([rgb(c) for _, c in RAMPS[ramp]])
    tinted = np.stack([np.interp(g, pos, cols[:, i]) for i in range(3)], axis=-1)
    return np.clip(tinted + rng.normal(0, 0.010, g.shape)[:, :, None], 0, 1)


def vignette(strength=0.55, radius=0.62):
    y, x = np.mgrid[0:SS, 0:SS] / (SS - 1) * 2 - 1
    d = np.hypot(x, y) / np.sqrt(2)
    return 1 - np.clip((d - radius) / (1 - radius), 0, 1) ** 1.6 * strength


def word_mask(weight, frac=0.74, word="sesh"):
    path = FONTS / f"JetBrainsMonoNerdFont-{weight}.ttf"
    probe = ImageFont.truetype(str(path), 64)
    box = probe.getbbox(word)
    font = ImageFont.truetype(str(path), int(64 * SS * frac / (box[2] - box[0])))
    mask = Image.new("L", (SS, SS), 0)
    ImageDraw.Draw(mask).text((SS / 2, SS / 2), word, font=font, fill=255, anchor="mm")
    return mask


def tube_mask(width=0.011):
    solid = word_mask("Regular")
    w = max(3, int(SS * width)) | 1
    ring = np.asarray(solid.filter(ImageFilter.MaxFilter(w)), np.int16) - \
        np.asarray(solid.filter(ImageFilter.MinFilter(w)), np.int16)
    return Image.fromarray(np.clip(ring, 0, 255).astype(np.uint8))


def screen(base, top):
    return 1 - (1 - base) * (1 - top)


def render(cloud, ramp, glow, darken=0.40):
    layers, core, kind = GLOWS[glow]
    mask = tube_mask() if kind == "tube" else word_mask("Bold")
    canvas = plate(cloud, ramp) * vignette()[:, :, None]

    backing = np.asarray(mask.filter(ImageFilter.GaussianBlur(SS * 0.14)), np.float64) / 255
    canvas *= 1 - (backing ** 0.55 * darken)[:, :, None]

    # sum the layers as light, so the hue survives instead of clipping to white
    light = np.zeros((SS, SS, 3))
    for radius, colour, amount in layers:
        blur = np.asarray(mask.filter(ImageFilter.GaussianBlur(SS * radius)), np.float64)
        light += (blur / 255 * amount)[:, :, None] * rgb(colour)
    canvas = screen(canvas, np.clip(light, 0, 1))

    lit = np.asarray(mask.filter(ImageFilter.GaussianBlur(SS * 0.0025)), np.float64) / 255
    a = (lit ** 1.6)[:, :, None]
    canvas = canvas * (1 - a) + rgb(core) * a

    img = Image.fromarray((np.clip(canvas, 0, 1) * 255).astype(np.uint8))
    return img.resize((FINAL, FINAL), Image.LANCZOS)


def install(name):
    cloud, ramp, glow = name.split("__")
    APPICON.mkdir(parents=True, exist_ok=True)
    render(cloud, ramp, glow).save(APPICON / "icon.png")
    (APPICON / "Contents.json").write_text(json.dumps({
        "images": [{"filename": "icon.png", "idiom": "universal",
                    "platform": "ios", "size": "1024x1024"}],
        "info": {"author": "xcode", "version": 1},
    }, indent=2) + "\n")
    print(f"installed {name} -> {APPICON.relative_to(ROOT)}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--install", metavar="NAME")
    ap.add_argument("--sheet", action="store_true")
    args = ap.parse_args()
    if args.install:
        return install(args.install)

    OUT.mkdir(parents=True, exist_ok=True)
    shots = []
    for variant in VARIANTS:
        name = "__".join(variant)
        img = render(*variant)
        img.save(OUT / f"{name}.png")
        shots.append(img)
        print(name)
    if args.sheet:
        cell, cols = 320, 5
        rows = -(-len(shots) // cols)
        sheet = Image.new("RGB", (cols * cell, rows * cell), "#0b0b10")
        for i, img in enumerate(shots):
            sheet.paste(img.resize((cell, cell), Image.LANCZOS),
                        (i % cols * cell, i // cols * cell))
        sheet.save(OUT / "sheet.png")
        print(f"sheet -> {(OUT / 'sheet.png').relative_to(ROOT)}")


if __name__ == "__main__":
    main()
