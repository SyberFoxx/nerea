#!/usr/bin/env python3
"""Build the Nerea brand assets from her own artwork and system fonts.

Reproducible on purpose: the logo, the social card and the signature are
generated, not hand-edited binaries. The README banner is the exception — it is
hand art, kept in the repository as `banner.png`, and the script never replaces
it by accident:

    python3 docs/brand/make_brand.py            # logo, social card, signature
    python3 docs/brand/make_brand.py --banner   # also rebuild the procedural banner
"""

from __future__ import annotations

import pathlib
import sys

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


def soften(alpha: np.ndarray, left_ramp: int = 0, bottom_ramp: int = 0) -> np.ndarray:
    """Fades a layer along its left edge and its bottom edge."""
    height, width = alpha.shape

    if left_ramp:
        ramp = np.clip(np.arange(width) / max(left_ramp, 1), 0, 1) ** 1.15
        alpha = np.minimum(alpha, np.tile((ramp * 255).astype(np.uint8)[None, :], (height, 1)))

    if bottom_ramp:
        ramp = np.clip((height - 1 - np.arange(height)) / max(bottom_ramp, 1), 0, 1) ** 1.25
        alpha = np.minimum(alpha, np.tile((ramp * 255).astype(np.uint8)[:, None], (1, width)))

    return alpha


def grain(image: Image.Image, amount: float = 4.5, seed: int = 7) -> Image.Image:
    """A little film grain: the cheapest way to stop a gradient looking printed."""
    array = np.array(image.convert("RGB"), float)
    noise = np.random.default_rng(seed).normal(0.0, amount, array.shape[:2])[..., None]
    return Image.fromarray(np.clip(array + noise, 0, 255).astype(np.uint8), "RGB")


def tracked_mask(size: tuple[int, int], xy: tuple[float, float], text: str,
                 font: ImageFont.FreeTypeFont, tracking: float) -> Image.Image:
    mask = Image.new("L", size, 0)
    pen = ImageDraw.Draw(mask)
    x, y = xy
    for character in text:
        pen.text((x, y), character, font=font, fill=255)
        x += pen.textlength(character, font=font) + tracking
    return mask


def build_banner() -> Image.Image:
    scale = 2
    width, height = 1600, 600
    w, h = width * scale, height * scale

    canvas = gradient(w, h, (11, 14, 18), (5, 7, 9))
    glow(canvas, w * 0.79, h * 0.26, w * 0.19, (88, 46, 164), 0.55)
    glow(canvas, w * 0.86, h * 0.00, w * 0.13, (20, 116, 106), 0.26)
    glow(canvas, w * 0.03, h * 1.24, w * 0.24, (22, 80, 76), 0.24)
    image = to_image(canvas).convert("RGBA")

    # Her name in kana, huge and almost invisible, behind everything.
    watermark = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    ImageDraw.Draw(watermark).text(
        (w * 0.47, h * 0.52),
        "ネレア",
        font=japanese(int(296 * scale)),
        fill=(138, 112, 220, 15),
        anchor="mm",
    )
    image.alpha_composite(watermark.filter(ImageFilter.GaussianBlur(2.6 * scale)))

    # One glass edge crossing the frame, so the background has a direction.
    streak = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    ImageDraw.Draw(streak).line(
        [(w * 0.16, h * 1.06), (w * 0.96, -h * 0.08)],
        fill=(176, 208, 255, 14),
        width=int(2 * scale),
    )
    image.alpha_composite(streak.filter(ImageFilter.GaussianBlur(int(7 * scale))))

    # She bleeds out of the frame: cropped at the top, melted at the bottom and
    # on her left side, so the painting does not read as a pasted rectangle.
    portrait = Image.open(ROOT / "ui" / "avatar" / "flirt.png").convert("RGBA")
    portrait = portrait.crop((140, 0, 780, 940))

    target_h = int(h * 1.16)
    target_w = round(portrait.width * target_h / portrait.height)
    portrait = portrait.resize((target_w, target_h), Image.LANCZOS)

    alpha = np.array(portrait.getchannel("A"))
    alpha = soften(
        alpha,
        left_ramp=int(target_w * 0.30),
        bottom_ramp=int(target_h * 0.16),
    )
    portrait.putalpha(Image.fromarray(alpha))

    px = w - target_w - int(w * 0.012)
    py = int(h * 0.02) - int(target_h - h * 0.97)
    image.alpha_composite(portrait, (px, max(py, -int(h * 0.12))))

    draw = ImageDraw.Draw(image, "RGBA")
    left = int(w * 0.061)

    tracked(
        draw,
        (left, int(h * 0.185)),
        "COMPAÑERA DE ESCRITORIO · LINUX",
        inter(int(13 * scale), SEMIBOLD),
        (104, 178, 170),
        tracking=3.6 * scale,
    )

    wordmark_font = inter(int(152 * scale), EXTRABOLD)
    wordmark_at = (left - int(4 * scale), int(h * 0.245))
    end_x = tracked(draw, wordmark_at, "nerea", wordmark_font, PAPER, tracking=3 * scale)

    # Gradient fill over the white so the wordmark has weight at the bottom.
    mask = tracked_mask((w, h), wordmark_at, "nerea", wordmark_font, 3 * scale)
    shade = gradient(w, h, (255, 255, 255), (203, 213, 221))
    image.paste(to_image(shade), (0, 0), mask)

    draw.text(
        (end_x + 14 * scale, int(h * 0.365)),
        "ネレア",
        font=japanese(int(34 * scale)),
        fill=(146, 128, 208, 235),
    )

    rule_y = int(h * 0.535)
    draw.rectangle(
        [left, rule_y, left + int(196 * scale), rule_y + int(4 * scale)],
        fill=TEAL,
    )



    tracked(
        draw,
        (left, int(h * 0.60)),
        "Habla con voz propia.",
        inter(int(32 * scale), SEMIBOLD),
        (233, 240, 245),
        tracking=0.2 * scale,
    )
    tracked(
        draw,
        (left, int(h * 0.695)),
        "Y recuerda lo que hablaron.",
        inter(int(32 * scale), LIGHT),
        (139, 156, 166),
        tracking=0.2 * scale,
    )

    dot = int(4 * scale)
    dot_y = int(h * 0.845)
    draw.ellipse([left, dot_y, left + dot * 2, dot_y + dot * 2], fill=TEAL)
    tracked(
        draw,
        (left + dot * 4, dot_y - int(3 * scale)),
        "v0.2.0 · Rust · Tauri 2 · AppImage x86_64",
        inter(int(18 * scale), MEDIUM),
        (116, 133, 143),
        tracking=1.6 * scale,
    )

    # Hairline frame and four corner ticks: it reads as a poster, not a crop.
    inset = int(16 * scale)
    draw.rectangle(
        [inset, inset, w - inset, h - inset],
        outline=(255, 255, 255, 14),
        width=scale,
    )
    tick, weight = int(26 * scale), int(2 * scale)
    for cx, cy, dx, dy in (
        (inset, inset, 1, 1),
        (w - inset, inset, -1, 1),
        (inset, h - inset, 1, -1),
        (w - inset, h - inset, -1, -1),
    ):
        draw.line([(cx, cy), (cx + dx * tick, cy)], fill=TEAL + (90,), width=weight)
        draw.line([(cx, cy), (cx, cy + dy * tick)], fill=TEAL + (90,), width=weight)

    return grain(image.convert("RGB").resize((width, height), Image.LANCZOS))


def build_social() -> Image.Image:
    """1280x640: the card GitHub shows when the repository is shared."""
    scale = 2
    width, height = 1280, 640
    w, h = width * scale, height * scale

    canvas = gradient(w, h, (11, 14, 18), (5, 7, 9))
    glow(canvas, w * 0.80, h * 0.28, w * 0.24, (88, 46, 164), 0.58)
    glow(canvas, w * 0.90, h * 0.02, w * 0.16, (20, 116, 106), 0.26)
    glow(canvas, w * 0.02, h * 1.20, w * 0.28, (22, 80, 76), 0.24)
    image = to_image(canvas).convert("RGBA")

    watermark = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    ImageDraw.Draw(watermark).text(
        (w * 0.52, h * 0.54),
        "ネレア",
        font=japanese(int(250 * scale)),
        fill=(138, 112, 220, 14),
        anchor="mm",
    )
    image.alpha_composite(watermark.filter(ImageFilter.GaussianBlur(2.6 * scale)))

    streak = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    ImageDraw.Draw(streak).line(
        [(w * 0.14, h * 1.08), (w * 0.98, -h * 0.10)],
        fill=(176, 208, 255, 14),
        width=int(2 * scale),
    )
    image.alpha_composite(streak.filter(ImageFilter.GaussianBlur(int(7 * scale))))

    portrait = Image.open(ROOT / "ui" / "avatar" / "flirt.png").convert("RGBA")
    portrait = portrait.crop((150, 0, 770, 960))

    target_h = int(h * 1.10)
    target_w = round(portrait.width * target_h / portrait.height)
    portrait = portrait.resize((target_w, target_h), Image.LANCZOS)

    alpha = np.array(portrait.getchannel("A"))
    alpha = soften(alpha, left_ramp=int(target_w * 0.26), bottom_ramp=int(target_h * 0.12))
    portrait.putalpha(Image.fromarray(alpha))
    image.alpha_composite(portrait, (w - target_w - int(w * 0.018), -int(h * 0.07)))

    draw = ImageDraw.Draw(image, "RGBA")
    left = int(w * 0.062)

    tracked(
        draw,
        (left, int(h * 0.150)),
        "COMPAÑERA DE ESCRITORIO · LINUX",
        inter(int(13 * scale), SEMIBOLD),
        (104, 178, 170),
        tracking=3.4 * scale,
    )

    wordmark_font = inter(int(126 * scale), EXTRABOLD)
    wordmark_at = (left - int(3 * scale), int(h * 0.205))
    end_x = tracked(draw, wordmark_at, "nerea", wordmark_font, PAPER, tracking=2.6 * scale)

    mask = tracked_mask((w, h), wordmark_at, "nerea", wordmark_font, 2.6 * scale)
    image.paste(to_image(gradient(w, h, (255, 255, 255), (203, 213, 221))), (0, 0), mask)

    draw.text(
        (end_x + 12 * scale, int(h * 0.315)),
        "ネレア",
        font=japanese(int(29 * scale)),
        fill=(146, 128, 208, 235),
    )

    rule_y = int(h * 0.475)
    draw.rectangle([left, rule_y, left + int(164 * scale), rule_y + int(4 * scale)], fill=TEAL)

    tracked(
        draw,
        (left, int(h * 0.535)),
        "Habla con voz propia.",
        inter(int(29 * scale), SEMIBOLD),
        (233, 240, 245),
        tracking=0.2 * scale,
    )
    tracked(
        draw,
        (left, int(h * 0.615)),
        "Y recuerda lo que hablaron.",
        inter(int(29 * scale), LIGHT),
        (139, 156, 166),
        tracking=0.2 * scale,
    )

    dot = int(4 * scale)
    dot_y = int(h * 0.775)
    draw.ellipse([left, dot_y, left + dot * 2, dot_y + dot * 2], fill=TEAL)
    tracked(
        draw,
        (left + dot * 4, dot_y - int(3 * scale)),
        "github.com/SyberFoxx/nerea",
        inter(int(17 * scale), MEDIUM),
        (116, 133, 143),
        tracking=1.2 * scale,
    )

    inset = int(15 * scale)
    draw.rectangle([inset, inset, w - inset, h - inset], outline=(255, 255, 255, 14), width=scale)
    tick, weight = int(24 * scale), int(2 * scale)
    for cx, cy, dx, dy in (
        (inset, inset, 1, 1),
        (w - inset, inset, -1, 1),
        (inset, h - inset, 1, -1),
        (w - inset, h - inset, -1, -1),
    ):
        draw.line([(cx, cy), (cx + dx * tick, cy)], fill=TEAL + (90,), width=weight)
        draw.line([(cx, cy), (cx, cy + dy * tick)], fill=TEAL + (90,), width=weight)

    return grain(image.convert("RGB").resize((width, height), Image.LANCZOS))


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

    # The banner is hand art (1440x720, the 2:1 GitHub ratio). Rebuilding the
    # procedural one is a deliberate move, never a side effect of regenerating
    # the rest of the brand.
    if "--banner" in sys.argv:
        build_banner().save(HERE / "banner.png")
        print("banner.png       rebuilt from the procedural recipe (the hand art is gone)")

    build_social().save(HERE / "social.jpg", quality=92, subsampling=0, optimize=True, progressive=True)
    build_signature().save(HERE / "signature.png")

    for name in ("logo.png", "logo-512.png", "banner.png", "social.jpg", "signature.png"):
        path = HERE / name
        if path.exists():
            print(f"{name:<16} {path.stat().st_size / 1024:7.0f} KB")


if __name__ == "__main__":
    main()
