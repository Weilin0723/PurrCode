#!/usr/bin/env python3
"""Derive the PurrCode production brand set from the supplied logo source.

PRD §25 requires a fixed `brand/` tree and forbids redesigning the mascot. This
script therefore *cuts* the mascot and the wordmark out of
`brand/purrcode-logo-source.png` rather than drawing new ones, then performs
only the production cleanup the PRD permits: removing the excess canvas the
source ships with, removing the raster noise around the transparent edge, and
producing the light-surface variant of the otherwise-white `Purr` wordmark.

Run from the repository root:

    python3 scripts/brand/build_brand_assets.py

Outputs are deterministic, so re-running must produce a byte-identical tree.
"""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
from PIL import Image

REPO = Path(__file__).resolve().parents[2]
BRAND = REPO / "brand"
SOURCE = BRAND / "purrcode-logo-source.png"

# The blue of `Code`, sampled from the source. `Purr` is white on dark surfaces
# and this slate on light ones, so the wordmark keeps its contrast either way.
ACCENT_BLUE = (95, 192, 250)
PURR_ON_DARK = (255, 255, 255)
PURR_ON_LIGHT = (30, 38, 51)

ICON_SIZES = (16, 24, 32, 48, 64, 128, 256, 512)


def load_source() -> Image.Image:
    if not SOURCE.exists():
        sys.exit(f"missing brand source: {SOURCE}")
    return Image.open(SOURCE).convert("RGBA")


def ink_mask(rgba: np.ndarray) -> np.ndarray:
    """True where the source actually has artwork.

    The source is a white-background render with a partly transparent canvas,
    so neither alpha alone nor colour alone identifies the drawing.
    """
    opaque = rgba[..., 3] > 8
    not_white = (rgba[..., :3] < 245).any(axis=-1)
    return opaque & not_white


def content_box(mask: np.ndarray) -> tuple[int, int, int, int]:
    ys, xs = np.nonzero(mask)
    return int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1


def whiten_to_alpha(region: Image.Image) -> Image.Image:
    """Turn the source's white paper into transparency.

    The mascot is drawn on white. Keeping that white would put a card behind
    the icon on every dark surface, so luminance becomes alpha for the pixels
    the source left fully white while the drawing itself is untouched.
    """
    array = np.array(region).astype(np.float32)
    rgb, alpha = array[..., :3], array[..., 3]
    lightness = rgb.min(axis=-1)
    # Fully white -> transparent, anything with ink -> opaque, with a short
    # ramp so anti-aliased glyph edges do not turn into a hard staircase.
    paper = np.clip((lightness - 236.0) / 14.0, 0.0, 1.0)
    array[..., 3] = np.minimum(alpha if alpha.max() > 0 else 255.0, (1.0 - paper) * 255.0)
    return Image.fromarray(array.clip(0, 255).astype(np.uint8), "RGBA")


def despeckle(image: Image.Image, min_alpha: int = 12) -> Image.Image:
    """Drop the near-transparent halo the source render leaves behind."""
    array = np.array(image)
    faint = array[..., 3] < min_alpha
    array[faint] = 0
    return Image.fromarray(array, "RGBA")


def trim(image: Image.Image) -> Image.Image:
    array = np.array(image)
    ys, xs = np.nonzero(array[..., 3] > 0)
    if len(xs) == 0:
        return image
    return image.crop((int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1))


def square(image: Image.Image, pad_ratio: float = 0.04) -> Image.Image:
    """Centre the artwork on a transparent square with a small optical margin."""
    width, height = image.size
    side = int(max(width, height) * (1.0 + pad_ratio * 2))
    canvas = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    canvas.paste(image, ((side - width) // 2, (side - height) // 2), image)
    return canvas


def dilate(mask: np.ndarray, radius: int) -> np.ndarray:
    grown = mask.copy()
    for _ in range(radius):
        padded = np.pad(grown, 1, constant_values=False)
        grown = (
            padded[1:-1, 1:-1]
            | padded[:-2, 1:-1]
            | padded[2:, 1:-1]
            | padded[1:-1, :-2]
            | padded[1:-1, 2:]
        )
    return grown


def erode(mask: np.ndarray, radius: int) -> np.ndarray:
    return ~dilate(~mask, radius)


def letterform_body(outline: np.ndarray, stroke: int) -> np.ndarray:
    """Recover solid glyphs from an outline-only drawing.

    `Purr` is drawn white on the source's white paper, so only its contour
    survives as ink — the letterforms themselves are paper, and so are the
    counters inside `P`, `u` and `r`. Filling everything the contour encloses
    would close those counters, so regions are classified by how many strokes
    separate them from the page: one stroke away is the glyph body, two is a
    counter that must stay open.
    """
    from scipy import ndimage

    sealed = dilate(outline, 2)
    regions, count = ndimage.label(~sealed)
    if count == 0:
        return outline
    border = set(regions[0].tolist()) | set(regions[-1].tolist())
    border |= set(regions[:, 0].tolist()) | set(regions[:, -1].tolist())
    border.discard(0)

    outside = np.isin(regions, list(border))
    # Reach across one contour — the stroke itself plus the sealing dilation on
    # both of its sides. Two contours away is a counter, which must stay open.
    near_page = dilate(outside, stroke * 2 + 4)
    body_ids = {
        label
        for label in range(1, count + 1)
        if label not in border
        and (regions == label).sum() > 4
        and (near_page & (regions == label)).any()
    }
    body = np.isin(regions, list(body_ids)) if body_ids else np.zeros_like(outline)
    return erode(body | sealed, 2) | outline


def stroke_width(outline: np.ndarray) -> int:
    """Median run length of the contour, used to size the region peeling."""
    runs: list[int] = []
    for row in outline:
        length = 0
        for value in row:
            if value:
                length += 1
            elif length:
                runs.append(length)
                length = 0
        if length:
            runs.append(length)
    return int(np.median(runs)) if runs else 3


def split_wordmark(
    source: Image.Image, box: tuple[int, int, int, int]
) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Separate the wordmark into its two words.

    Returns the solid `Purr` mask, the `Code` mask, and `Code`'s original
    pixels. The blue word is kept exactly as drawn; the white one only exists
    in the source as a contour, so it is reconstructed.
    """
    crop = np.array(source.crop(box)).astype(np.int16)
    rgb = crop[..., :3]
    spread = rgb.max(axis=-1) - rgb.min(axis=-1)
    ink = (rgb < 245).any(axis=-1) & (crop[..., 3] > 8)
    code = ink & (spread >= 30)
    outline = ink & ~code
    purr = letterform_body(outline, stroke_width(outline))
    return purr, code, crop[..., :3].astype(np.uint8)


def compose_wordmark(
    purr: np.ndarray,
    code: np.ndarray,
    code_pixels: np.ndarray,
    purr_colour: tuple[int, int, int],
) -> Image.Image:
    out = np.zeros((purr.shape[0], purr.shape[1], 4), dtype=np.uint8)
    out[purr, :3] = purr_colour
    out[purr, 3] = 255
    out[code, :3] = code_pixels[code]
    out[code, 3] = 255
    return Image.fromarray(out, "RGBA")


def trace_paths(mask: np.ndarray, tolerance: float = 0.35) -> list[list[tuple[float, float]]]:
    """Turn a glyph mask into simplified closed contours.

    Contours are traced at the half-pixel isoline and then reduced with
    Douglas-Peucker, so the result is a genuine outline rather than one
    rectangle per pixel. `render_svg_preview` exists so the output is reviewed
    against the source instead of being shipped on trust (PRD §21.2).
    """
    from skimage import measure

    padded = np.pad(mask.astype(float), 1)
    paths: list[list[tuple[float, float]]] = []
    for contour in measure.find_contours(padded, 0.5):
        simplified = measure.approximate_polygon(contour, tolerance=tolerance)
        if len(simplified) < 4:
            continue
        # find_contours works in pixel-centre coordinates and SVG fills between
        # pixel edges; the half-pixel shift is what keeps the traced glyph the
        # same weight as the drawn one rather than a hairline thinner.
        paths.append([(float(x - 0.5), float(y - 0.5)) for y, x in simplified])
    return paths


def paths_to_d(paths: list[list[tuple[float, float]]]) -> str:
    parts: list[str] = []
    for path in paths:
        points = " L".join(f"{x:.2f} {y:.2f}" for x, y in path[1:])
        parts.append(f"M{path[0][0]:.2f} {path[0][1]:.2f} L{points} Z")
    return " ".join(parts)


def rasterize(
    paths: list[list[tuple[float, float]]], shape: tuple[int, int]
) -> np.ndarray:
    """Even-odd scanline fill, used to check a trace against its source mask."""
    height, width = shape
    out = np.zeros(shape, dtype=bool)
    edges = [
        (x0, y0, x1, y1)
        for path in paths
        for (x0, y0), (x1, y1) in zip(path, path[1:] + path[:1])
        if y0 != y1
    ]
    for row in range(height):
        y = row + 0.5
        crossings = sorted(
            x0 + (y - y0) * (x1 - x0) / (y1 - y0)
            for x0, y0, x1, y1 in edges
            if min(y0, y1) <= y < max(y0, y1)
        )
        for start, end in zip(crossings[0::2], crossings[1::2]):
            lo = max(0, int(round(start)))
            hi = min(width, int(round(end)))
            if hi > lo:
                out[row, lo:hi] = True
    return out


def trace_fidelity(mask: np.ndarray, paths: list[list[tuple[float, float]]]) -> float:
    """Intersection-over-union between a glyph mask and its traced outline."""
    traced = rasterize(paths, mask.shape)
    union = (mask | traced).sum()
    return float((mask & traced).sum() / union) if union else 1.0


def write_wordmark_svg(
    destination: Path,
    purr: np.ndarray,
    code: np.ndarray,
    purr_colour: tuple[int, int, int],
) -> None:
    height, width = purr.shape
    purr_paths, code_paths = trace_paths(purr), trace_paths(code)
    for word, mask, paths in (("Purr", purr, purr_paths), ("Code", code, code_paths)):
        fidelity = trace_fidelity(mask, paths)
        if fidelity < 0.97:
            sys.exit(f"{word} traced at {fidelity:.3f} IoU — review before shipping")
        print(f"  {destination.name}: {word} trace fidelity {fidelity:.4f}")
    purr_d = paths_to_d(purr_paths)
    code_d = paths_to_d(code_paths)
    accent = "#%02x%02x%02x" % ACCENT_BLUE
    fill = "#%02x%02x%02x" % purr_colour
    destination.write_text(
        "<!-- PurrCode wordmark. Traced from brand/purrcode-logo-source.png by\n"
        "     scripts/brand/build_brand_assets.py and reviewed against the source. -->\n"
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}"'
        f' width="{width}" height="{height}" role="img" aria-label="PurrCode">\n'
        f'  <path fill="{fill}" fill-rule="evenodd" d="{purr_d}"/>\n'
        f'  <path fill="{accent}" fill-rule="evenodd" d="{code_d}"/>\n'
        "</svg>\n",
        encoding="utf-8",
    )


def write_cat_head_svgs(mascot: Image.Image) -> None:
    """Derive the small-size cat-head icons from the mascot itself.

    PRD §21.2 allows simplifying detail that cannot survive icon sizes but
    forbids redesigning the mascot, so the shapes come from the artwork:
    the head is posterised to a handful of brand colours, each colour region is
    traced, and regions too small to read at 16px are dropped. The monochrome
    variant keeps only the silhouette, the ears, and the eyes — the three
    features that still identify the cat with no colour at all.
    """
    from scipy import ndimage

    side = 256
    art = square(mascot, pad_ratio=0.01).resize((side, side), Image.LANCZOS)
    array = np.array(art)
    alpha = array[..., 3] > 110
    rgb = array[..., :3].astype(np.int16)

    # A ragdoll reduced to what a 16px icon can hold: the dark mask over the
    # ears and brow, the cream ruff, the blue eyes, the purple collar.
    palette = {
        "#3f3229": np.array([70, 58, 50]),
        "#8d7c6c": np.array([150, 133, 116]),
        "#efe7dc": np.array([238, 231, 220]),
        "#2f8fd8": np.array([60, 150, 215]),
        "#1b1b28": np.array([30, 30, 40]),
        "#5b3f8f": np.array([95, 65, 145]),
    }
    names = list(palette)
    stack = np.stack([palette[name] for name in names])
    distance = np.linalg.norm(rgb[:, :, None, :] - stack[None, None, :, :], axis=-1)
    nearest = distance.argmin(axis=-1)

    layers: list[tuple[str, str]] = []
    for index, name in enumerate(names):
        mask = alpha & (nearest == index)
        labelled, count = ndimage.label(mask)
        if count:
            keep = [
                label
                for label in range(1, count + 1)
                if (labelled == label).sum() >= 90
            ]
            mask = np.isin(labelled, keep) if keep else np.zeros_like(mask)
        mask = ndimage.binary_closing(mask, np.ones((3, 3)))
        if not mask.any():
            continue
        paths = trace_paths(mask, tolerance=1.1)
        if paths:
            layers.append((name, paths_to_d(paths)))

    body = "\n".join(
        f'  <path fill="{colour}" fill-rule="evenodd" d="{d}"/>' for colour, d in layers
    )
    (BRAND / "purrcode-cat-head.svg").write_text(
        "<!-- PurrCode cat head. Traced from brand/purrcode-logo-source.png by\n"
        "     scripts/brand/build_brand_assets.py; detail below icon legibility\n"
        "     is dropped, the mascot itself is not redrawn (PRD §21.2). -->\n"
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {side} {side}"'
        f' width="{side}" height="{side}" role="img" aria-label="PurrCode">\n'
        f"{body}\n</svg>\n",
        encoding="utf-8",
    )

    # Individual fur strands read as noise below about 32px, so the silhouette
    # is closed before tracing and traced loosely: the ears and the ruff still
    # come through, the strands do not.
    silhouette = ndimage.binary_fill_holes(
        ndimage.binary_closing(
            ndimage.median_filter(alpha, size=7), np.ones((9, 9))
        )
    )
    # The eyes are the only strongly blue thing in the upper half of the
    # artwork — the collar tag is blue too, which is why the search is bounded.
    # Taking them by hue rather than by nearest-palette keeps each eye whole:
    # pupil, iris and highlight instead of three posterised fragments.
    blue = alpha & (rgb[..., 2] - rgb[..., 0] > 20) & (rgb[..., 2] > 70)
    blue = ndimage.binary_fill_holes(ndimage.binary_closing(blue, np.ones((5, 5))))
    labelled, count = ndimage.label(blue)
    candidates = []
    for label in range(1, count + 1):
        component = labelled == label
        size_px = int(component.sum())
        centre = float(np.nonzero(component)[0].mean())
        if size_px >= 200 and centre < side * 0.55:
            candidates.append((size_px, label))
    keep = [label for _, label in sorted(candidates, reverse=True)[:2]]
    eyes = np.isin(labelled, keep) if keep else np.zeros_like(blue)
    # One even-odd path: the silhouette filled, the eyes knocked out of it. A
    # solid blob is not a cat, and a 16px monochrome icon has room for nothing
    # else.
    knockout = paths_to_d(trace_paths(silhouette, 2.4) + trace_paths(eyes, 1.0))
    (BRAND / "purrcode-cat-head-monochrome.svg").write_text(
        "<!-- PurrCode cat head, single colour. currentColor lets a 16px UI, a\n"
        "     high-contrast theme and terminal documentation share one file. -->\n"
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {side} {side}"'
        f' width="{side}" height="{side}" role="img" aria-label="PurrCode">\n'
        f'  <path fill="currentColor" fill-rule="evenodd" d="{knockout}"/>\n'
        "</svg>\n",
        encoding="utf-8",
    )


def lockup(mascot: Image.Image, word: Image.Image, gap_ratio: float = 0.10) -> Image.Image:
    """Compose the horizontal logo at the proportions the source uses."""
    height = max(mascot.height, word.height)
    gap = int(mascot.width * gap_ratio)
    canvas = Image.new("RGBA", (mascot.width + gap + word.width, height), (0, 0, 0, 0))
    canvas.paste(mascot, (0, (height - mascot.height) // 2), mascot)
    canvas.paste(word, (mascot.width + gap, (height - word.height) // 2), word)
    return canvas


def main() -> None:
    source = load_source()
    array = np.array(source)
    mask = ink_mask(array)

    # The source places the mascot and the wordmark on one row with a clear
    # column of untouched paper between them; split on that gap rather than on
    # hard-coded pixel offsets so a re-export of the source still works.
    left, top, right, bottom = content_box(mask)
    columns = mask[top:bottom, left:right].sum(axis=0)
    empty = np.nonzero(columns == 0)[0]
    runs: list[tuple[int, int]] = []
    if len(empty):
        start = prev = empty[0]
        for value in empty[1:]:
            if value != prev + 1:
                runs.append((start, prev))
                start = value
            prev = value
        runs.append((start, prev))
    widest = max(runs, key=lambda run: run[1] - run[0], default=None)
    if widest is None:
        sys.exit("could not separate the mascot from the wordmark")
    split = left + (widest[0] + widest[1]) // 2

    mascot = despeckle(trim(whiten_to_alpha(source.crop((left, top, split, bottom)))))
    word_box = (split, top, right, bottom)
    purr_mask, code_mask, code_pixels = split_wordmark(source, word_box)
    used = purr_mask | code_mask
    ys, xs = np.nonzero(used)
    tight = (slice(ys.min(), ys.max() + 1), slice(xs.min(), xs.max() + 1))
    purr_mask, code_mask = purr_mask[tight], code_mask[tight]
    code_pixels = code_pixels[tight]
    word_on_dark = compose_wordmark(purr_mask, code_mask, code_pixels, PURR_ON_DARK)
    word_on_light = compose_wordmark(purr_mask, code_mask, code_pixels, PURR_ON_LIGHT)

    BRAND.mkdir(parents=True, exist_ok=True)
    (BRAND / "icons").mkdir(exist_ok=True)

    mascot_square = square(mascot)
    mascot_square.resize((1024, 1024), Image.LANCZOS).save(BRAND / "purrcode-mascot-large.png")

    for size in ICON_SIZES:
        # Small sizes lose the collar tag's inner detail; the resample keeps the
        # silhouette and the eyes, which is what has to stay recognisable.
        mascot_square.resize((size, size), Image.LANCZOS).save(BRAND / "icons" / f"{size}.png")

    target_height = 320
    for word, name in ((word_on_dark, "dark"), (word_on_light, "light")):
        image = lockup(mascot, word)
        scale = target_height / image.height
        image.resize(
            (max(1, round(image.width * scale)), target_height), Image.LANCZOS
        ).save(BRAND / f"purrcode-logo-horizontal-{name}.png")

        scale = 160 / word.height
        word.resize((max(1, round(word.width * scale)), 160), Image.LANCZOS).save(
            BRAND / f"purrcode-wordmark-{name}.png"
        )

    for colour, name in ((PURR_ON_DARK, "dark"), (PURR_ON_LIGHT, "light")):
        write_wordmark_svg(
            BRAND / f"purrcode-wordmark-{name}.svg", purr_mask, code_mask, colour
        )
    write_cat_head_svgs(mascot)

    print(f"mascot {mascot.size} wordmark {word_on_dark.size} -> brand/")


if __name__ == "__main__":
    main()
