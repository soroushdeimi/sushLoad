#!/usr/bin/env python3
"""Turns the full-bleed artwork (assets/logo.png) into a macOS app icon: the artwork is clipped to
Apple's 824px rounded-square grid inside a 1024px canvas with a soft drop shadow, then written as
assets/AppIcon.png and assets/AppIcon.icns. Needs Pillow; only run when the logo changes."""
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "assets" / "logo.png"
OUT_PNG = ROOT / "assets" / "AppIcon.png"
OUT_ICNS = ROOT / "assets" / "AppIcon.icns"

CANVAS, BODY, RADIUS = 1024, 824, 185
SS = 4  # supersampling for a smooth mask edge


def rounded_mask(size, radius):
    big = Image.new("L", (size * SS, size * SS), 0)
    ImageDraw.Draw(big).rounded_rectangle(
        (0, 0, size * SS - 1, size * SS - 1), radius=radius * SS, fill=255
    )
    return big.resize((size, size), Image.LANCZOS)


def main():
    art = Image.open(SRC).convert("RGBA").resize((BODY, BODY), Image.LANCZOS)
    mask = rounded_mask(BODY, RADIUS)
    off = (CANVAS - BODY) // 2

    shadow = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    shadow_body = Image.new("RGBA", (BODY, BODY), (0, 0, 0, 120))
    shadow.paste(shadow_body, (off, off + 12), mask)
    icon = shadow.filter(ImageFilter.GaussianBlur(14))
    icon.paste(art, (off, off), mask)
    icon.save(OUT_PNG)

    with tempfile.TemporaryDirectory() as tmp:
        iconset = Path(tmp) / "AppIcon.iconset"
        iconset.mkdir()
        for s in (16, 32, 128, 256, 512):
            icon.resize((s, s), Image.LANCZOS).save(iconset / f"icon_{s}x{s}.png")
            icon.resize((s * 2, s * 2), Image.LANCZOS).save(iconset / f"icon_{s}x{s}@2x.png")
        if not shutil.which("iconutil"):
            sys.exit("iconutil not found (macOS only)")
        subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(OUT_ICNS)], check=True)
    print(f"wrote {OUT_PNG.relative_to(ROOT)} and {OUT_ICNS.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
