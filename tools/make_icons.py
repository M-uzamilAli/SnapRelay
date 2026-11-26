"""Make Windows .ico files from the brand icons.

    python tools/make_icons.py

Reads docs/assets/brand/snaprelay-icon-{dark,light}.webp (1024x1024, rounded
square on a transparent canvas with a soft shadow) and writes
assets/icons/snaprelay-{dark,light}.ico with the sizes Windows uses
(16-256 px). The shadow is dropped and the square cropped tight, so the
icon stays legible at tray size (16-24 px).
"""
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "docs" / "assets" / "brand"
OUT = ROOT / "assets" / "icons"
SIZES = [(16, 16), (20, 20), (24, 24), (32, 32), (40, 40), (48, 48), (64, 64), (128, 128), (256, 256)]


def tight_square(img: Image.Image) -> Image.Image:
    # The solid rounded square: pixels that are clearly opaque.
    alpha = img.getchannel("A")
    solid = alpha.point(lambda a: 255 if a > 200 else 0)
    left, top, right, bottom = solid.getbbox()
    side = max(right - left, bottom - top)
    cx, cy = (left + right) // 2, (top + bottom) // 2
    box = (cx - side // 2, cy - side // 2, cx + side // 2, cy + side // 2)
    square = img.crop(box)
    # Drop the faint drop-shadow halo around the square.
    a = square.getchannel("A").point(lambda v: 0 if v < 60 else v)
    square.putalpha(a)
    return square


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    for theme in ("dark", "light"):
        src = Image.open(SRC / f"snaprelay-icon-{theme}.webp").convert("RGBA")
        icon = tight_square(src).resize((256, 256), Image.LANCZOS)
        dest = OUT / f"snaprelay-{theme}.ico"
        icon.save(dest, sizes=SIZES)
        print(f"wrote {dest.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
