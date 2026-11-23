#!/usr/bin/env python3
"""Build the Nerea brand assets from her own artwork and system fonts.

Reproducible on purpose: the logo, the README banner and the signature are
generated, not hand-edited binaries. Run it after changing the artwork:

    python3 docs/brand/make_brand.py
"""

from __future__ import annotations

import pathlib

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[1]

INTER = "/usr/share/fonts/inter/Inter.ttc"
SERIF_JP = "/usr/share/fonts/noto-cjk/NotoSerifCJK-Bold.ttc"

REGULAR, LIGHT, MEDIUM, SEMIBOLD, BOLD, EXTRABOLD = 0, 6, 10, 12, 14, 16

VIOLET = (139, 92, 246)
VIOLET_DEEP = (91, 33, 182)
TEAL = (46, 196, 182)
INK = (11, 15, 19)
PAPER = (240, 246, 250)
MUTED = (141, 163, 173)
RADIUS = 224


def inter(size: int, weight: int = REGULAR) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(INTER, size, index=weight)


def japanese(size: int) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(SERIF_JP, size, index=0)


def gradient(width: int, height: int, top: tuple, bottom: tuple) -> np.ndarray:
    start, end = np.array(top, float), np.array(bottom, float)
    ramp = np.linspace(0.0, 1.0, height)[:, None, None]
    return np.broadcast_to(start + (end - start) * ramp, (height, width, 3)).copy()


def glow(canvas: np.ndarray, cx: float, cy: float, radius: float, colour: tuple, strength: float) -> None:
    height, width = canvas.shape[:2]
    yy, xx = np.mgrid[0:height, 0:width]
    distance = ((xx - cx) ** 2 + (yy - cy) ** 2) / (radius**2)
    canvas += np.exp(-distance)[..., None] * np.array(colour, float) * strength


def to_image(canvas: np.ndarray) -> Image.Image:
    return Image.fromarray(np.clip(canvas, 0, 255).astype(np.uint8), "RGB")


def rounded(size: tuple[int, int], radius: int) -> Image.Image:
    mask = Image.new("L", size, 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [0, 0, size[0] - 1, size[1] - 1], radius=radius, fill=255
    )
    return mask


def lens(cx: float, cy: float, width: float, height: float, steps: int = 480) -> list[tuple[float, float]]:
    """An almond eye: two mirrored arcs meeting in sharp corners."""
    points: list[tuple[float, float]] = []
    for index in range(steps):
        x = -1 + 2 * index / (steps - 1)
        y = (1 - abs(x) ** 1.7) ** 0.6
        points.append((cx + x * width / 2, cy - y * height / 2))
    for index in range(steps):
        x = 1 - 2 * index / (steps - 1)
        y = (1 - abs(x) ** 1.7) ** 0.6
        points.append((cx + x * width / 2, cy + y * height / 2))
    return points


def tracked(draw: ImageDraw.ImageDraw, xy: tuple[float, float], text: str,
            font: ImageFont.FreeTypeFont, fill: tuple, tracking: float = 0.0) -> float:
    x, y = xy
    for character in text:
        draw.text((x, y), character, font=font, fill=fill)
        x += draw.textlength(character, font=font) + tracking
    return x


def build_logo() -> Image.Image:
    scale, side = 4, 1024
    size = side * scale

    canvas = gradient(size, size, (21, 29, 36), (7, 10, 14))
    glow(canvas, size * 0.20, size * 0.92, size * 0.52, VIOLET_DEEP, 0.34)
    glow(canvas, size * 0.86, size * 0.10, size * 0.44, (10, 88, 88), 0.52)
    image = to_image(canvas).convert("RGBA")

    cx = cy = size / 2
    width, height = size * 0.60, size * 0.38
    outline = lens(cx, cy, width, height)

    # Iris: a violet radial field clipped to the eye shape.
    yy, xx = np.mgrid[0:size, 0:size]
    distance = np.sqrt((xx - cx) ** 2 + (yy - cy) ** 2) / (width * 0.62)
    mix = np.clip(1 - distance, 0, 1)[..., None]
    iris = np.array([206, 186, 255], float) * mix + np.array([52, 18, 110], float) * (1 - mix)

    # Eyelid shadow: the top of the eye sits in its own shade.
    shade = np.clip(1 - (yy - (cy - height * 0.5)) / (height * 0.55), 0, 1)[..., None]
    iris *= 1 - 0.35 * shade
    iris_image = to_image(iris)

    eye_mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(eye_mask).polygon(outline, fill=255)
    image.paste(iris_image, (0, 0), eye_mask)

    draw = ImageDraw.Draw(image, "RGBA")

    # Slit pupil: she is a demon, and it reads at 32 px.
    pupil_w, pupil_h = width * 0.085, height * 0.66
    draw.ellipse(
        [cx - pupil_w / 2, cy - pupil_h / 2, cx + pupil_w / 2, cy + pupil_h / 2],
        fill=(6, 9, 12, 255),
    )

    # Highlight, slightly off-centre like real glass.
    highlight = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    ImageDraw.Draw(highlight).ellipse(
        [
            cx - width * 0.175,
            cy - height * 0.27,
            cx - width * 0.075,
            cy - height * 0.09,
        ],
        fill=(255, 255, 255, 235),
    )
    image.alpha_composite(highlight.filter(ImageFilter.GaussianBlur(size * 0.0035)))

    # Teal rim plus its outer bloom.
    bloom = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    ImageDraw.Draw(bloom).line(outline + [outline[0]], fill=TEAL + (200,), width=int(size * 0.016), joint="curve")
    image.alpha_composite(bloom.filter(ImageFilter.GaussianBlur(size * 0.018)))
    draw.line(outline + [outline[0]], fill=(120, 232, 224, 235), width=int(size * 0.010), joint="curve")

    # Frame.
    draw.rounded_rectangle(
        [0, 0, size - 1, size - 1],
        radius=222 * scale,
        outline=(46, 196, 182, 70),
        width=scale * 2,
    )

    image.putalpha(rounded((size, size), 222 * scale))
    return image.resize((side, side), Image.LANCZOS)


def build_portrait_logo() -> Image.Image:
    """Her face in the badge: the app is a person, not an abstract mark."""
    side = 1024
    source = Image.open(ROOT / "ui" / "avatar" / "flirt.png").convert("RGBA")
    face = source.crop((196, 0, 664, 580))

    cover = max(side / face.width, side / face.height)
    scaled = face.resize((round(face.width * cover), round(face.height * cover)), Image.LANCZOS)
    offset = ((side - scaled.width) // 2, 0)
    badge = Image.new("RGBA", (side, side), (11, 15, 19, 255))
    badge.alpha_composite(scaled, offset)

    # Vignette so the crop melts into the frame instead of ending in a hard edge.
    yy, xx = np.mgrid[0:side, 0:side]
    edge = np.sqrt(((xx - side / 2) / (side / 2)) ** 2 + ((yy - side / 2) / (side / 2)) ** 2)
    vignette = np.clip((edge - 0.62) / 0.55, 0, 1) ** 1.5
    shade = Image.new("RGBA", (side, side), (7, 10, 14, 255))
    badge = Image.composite(shade, badge, Image.fromarray((vignette * 235).astype(np.uint8)))

    frame = ImageDraw.Draw(badge, "RGBA")
    badge.putalpha(rounded((side, side), RADIUS))
    frame = ImageDraw.Draw(badge, "RGBA")
    frame.rounded_rectangle(
        [3, 3, side - 4, side - 4],
        radius=RADIUS - 3,
        outline=(46, 196, 182, 90),
        width=4,
    )
    return badge


def build_banner() -> Image.Image:
    scale = 2
    width, height = 1600, 560
    w, h = width * scale, height * scale

    canvas = gradient(w, h, (12, 17, 21), (7, 10, 13))
    glow(canvas, w * 0.16, h * 1.10, w * 0.40, VIOLET_DEEP, 0.34)
    glow(canvas, w * 0.88, h * 0.06, w * 0.32, (9, 70, 70), 0.46)
    image = to_image(canvas).convert("RGBA")

    portrait = Image.open(ROOT / "ui" / "avatar" / "flirt.png").convert("RGBA")
    target_h = int(h * 0.93)
    target_w = round(portrait.width * target_h / portrait.height)
    portrait = portrait.resize((target_w, target_h), Image.LANCZOS)

    backlight = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    ImageDraw.Draw(backlight).ellipse(
        [
            w - target_w * 0.95,
            h * 0.12,
            w + target_w * 0.35,
            h * 1.05,
        ],
        fill=(120, 70, 210, 90),
    )
    image.alpha_composite(backlight.filter(ImageFilter.GaussianBlur(int(w * 0.035))))
    image.alpha_composite(portrait, (w - target_w - int(w * 0.012), h - target_h))

    draw = ImageDraw.Draw(image, "RGBA")
    left = int(w * 0.058)

    wordmark = inter(int(132 * scale), EXTRABOLD)
    tracked(draw, (left, int(h * 0.20)), "nerea", wordmark, PAPER, tracking=6 * scale)

    draw.rectangle(
        [left, int(h * 0.505), left + int(148 * scale), int(h * 0.505) + 5 * scale],
        fill=TEAL,
    )

    tracked(
        draw,
        (left, int(h * 0.565)),
        "Compañera de escritorio con voz propia",
        inter(int(30 * scale), MEDIUM),
        (208, 221, 228),
        tracking=0.4 * scale,
    )
    tracked(
        draw,
        (left, int(h * 0.655)),
        "Rust · Tauri 2 · Linux",
        inter(int(23 * scale), LIGHT),
        MUTED,
        tracking=2.2 * scale,
    )

    chip = "v0.2.0"
    chip_font = inter(int(21 * scale), SEMIBOLD)
    chip_w = draw.textlength(chip, font=chip_font) + 26 * scale
    chip_box = [
        left,
        int(h * 0.815),
        left + chip_w,
        int(h * 0.815) + int(38 * scale),
    ]
    draw.rounded_rectangle(chip_box, radius=int(10 * scale), outline=(46, 196, 182, 120), width=2 * scale)
    draw.text(
        (chip_box[0] + 13 * scale, chip_box[1] + 8 * scale),
        chip,
        font=chip_font,
        fill=(150, 226, 218),
    )

    return image.resize((width, height), Image.LANCZOS)


def build_signature() -> Image.Image:
    scale = 4
    width, height = 760, 220
    w, h = width * scale, height * scale
    image = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image, "RGBA")

    name = "ネレア"
    font = japanese(int(126 * scale))
    box = draw.textbbox((0, 0), name, font=font)
    x = (w - (box[2] - box[0])) / 2
    draw.text((x, int(10 * scale)), name, font=font, fill=(236, 243, 248, 255))

    # A teal flourish under the name, fading at both ends.
    for index in range(140):
        t = index / 139
        fade = int(200 * (1 - abs(2 * t - 1) ** 1.6))
        px = int(w * 0.22) + t * w * 0.56
        py = int(h * 0.80) + (1 - abs(2 * t - 1) ** 1.4) * -6 * scale
        draw.ellipse([px, py, px + 3 * scale, py + 3 * scale], fill=TEAL + (fade,))

    return image.resize((width, height), Image.LANCZOS)


def main() -> None:
    HERE.mkdir(parents=True, exist_ok=True)

    logo = build_portrait_logo()
    logo.save(HERE / "logo.png")
    logo.resize((512, 512), Image.LANCZOS).save(HERE / "logo-512.png")

    build_banner().save(HERE / "banner.png")
    build_signature().save(HERE / "signature.png")

    for name in ("logo.png", "logo-512.png", "banner.png", "signature.png"):
        path = HERE / name
        print(f"{name:<16} {path.stat().st_size / 1024:7.0f} KB")


if __name__ == "__main__":
    main()
