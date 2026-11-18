"""What re-rendering does to a picture, generation by generation (se-153, proof only).

    rerender.py --plan PLAN.json --seq DIR --out TSV     DIR/<sequence>/g<k>.<ext>, g0 the sequence's start
    rerender.py --self-check PICTURE

Per sequence and generation, against generation 0 (the picture the sequence starts from):
- same: the share of pixels within one 8-bit level of the generation before, when they are one size (what paste-back kept).
- per box (the sequence's `boxes`, fractions of the picture), of luminance at 100%:
  lap    the Laplacian's variance;
  lap/2  the same after 2x2 averaging (blind to a 2-px pattern);
  bands  spectral power at 1/16-1/8, 1/8-1/4 and 1/4-3/8 cycles a pixel;
  sub2   the decoder's 2-px pattern (se-155): the 2x2 sublattice's mean |offset|, 8-bit levels, absolute.
A sequence without boxes is measured over its whole picture. When a generation changes size (a new framing) nothing
compares pixel to pixel: lap and lap/2 are absolute and each band is its percentage of the box's spectral power.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np
import numpy.typing as npt
from PIL import Image

Floats = npt.NDArray[np.float64]


def luma(image: Image.Image, box: list[float]) -> Floats:
    if "A" in image.getbands():
        white = Image.new("RGBA", image.size, "white")
        white.alpha_composite(image.convert("RGBA"))
        image = white
    w, h = image.size
    region = (round(box[0] * w), round(box[1] * h), round(box[2] * w), round(box[3] * h))
    return np.asarray(image.convert("L").crop(region), dtype=np.float64)


def laplacian(a: Floats) -> float:
    return float((-4 * a[1:-1, 1:-1] + a[:-2, 1:-1] + a[2:, 1:-1] + a[1:-1, :-2] + a[1:-1, 2:]).var())


def halved(a: Floats) -> Floats:
    h, w = a.shape[0] // 2 * 2, a.shape[1] // 2 * 2
    return a[:h, :w].reshape(h // 2, 2, w // 2, 2).mean((1, 3))


def bands(a: Floats) -> list[float]:
    """The three bands' power, then all of it."""
    b = (a - a.mean()) * np.outer(np.hanning(a.shape[0]), np.hanning(a.shape[1]))
    power = np.abs(np.fft.fft2(b)) ** 2
    fy, fx = np.meshgrid(np.fft.fftfreq(a.shape[0]), np.fft.fftfreq(a.shape[1]), indexing="ij")
    r = np.hypot(fx, fy)
    return [*(float(power[(r >= lo) & (r < hi)].sum()) for lo, hi in ((1 / 16, 1 / 8), (1 / 8, 1 / 4), (1 / 4, 3 / 8))), float(power.sum())]


def sub2(a: Floats) -> float:
    pad = np.pad(a, ((2, 1), (2, 1)), mode="reflect")
    c = np.pad(pad.cumsum(0).cumsum(1), ((1, 0), (1, 0)))
    box = (c[4:, 4:] - c[:-4, 4:] - c[4:, :-4] + c[:-4, :-4])[: a.shape[0], : a.shape[1]] / 16
    hp = a - box
    m = np.array([[hp[i::2, j::2].mean() for j in range(2)] for i in range(2)])
    return float(np.abs(m - m.mean()).mean())


def measures(image: Image.Image, box: list[float]) -> list[float]:
    a = luma(image, box)
    """lap, lap/2, the three bands, all spectral power, sub2."""
    return [laplacian(a), laplacian(halved(a)), *bands(a), sub2(a)]


def same(a: Image.Image, b: Image.Image) -> float | None:
    if a.size != b.size:
        return None
    x, y = np.asarray(a.convert("RGB"), dtype=np.int16), np.asarray(b.convert("RGB"), dtype=np.int16)
    return float((np.abs(x - y).max(axis=-1) <= 1).mean())


def self_check(picture: str) -> int:
    """A picture against itself is 1.00 everywhere; an unsharp mask of it raises the Laplacian and every band."""
    image = Image.open(picture).convert("RGB")
    box = [0.25, 0.25, 0.75, 0.75]
    first = measures(image, box)
    a = np.asarray(image, dtype=np.float64)
    blur = (a + np.roll(a, 1, 0) + np.roll(a, -1, 0) + np.roll(a, 1, 1) + np.roll(a, -1, 1)) / 5
    sharp = Image.fromarray(np.clip(a + 0.8 * (a - blur), 0, 255).astype(np.uint8))
    ratio = [x / y for x, y in zip(measures(sharp, box)[:5], first[:5])]
    print("unsharp mask against the picture:", " ".join(f"{v:.2f}" for v in ratio), "same", same(image, image))
    return 0 if all(v > 1.2 for v in ratio) and same(image, image) == 1.0 else 1


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--plan")
    p.add_argument("--seq")
    p.add_argument("--out")
    p.add_argument("--self-check")
    args = p.parse_args()
    if args.self_check:
        return self_check(args.self_check)
    rows = ["sequence\tgen\tsize\tsame\tbox\tlap\tlap/2\t1/16-1/8\t1/8-1/4\t1/4-3/8\tsub2"]
    for q in json.load(open(args.plan)):
        folder = Path(args.seq) / q["name"]
        files = sorted(folder.glob("g*.*"), key=lambda f: int(f.stem[1:]))
        if not files:
            continue
        boxes: dict[str, list[float]] = q.get("boxes") or {"whole": [0.0, 0.0, 1.0, 1.0]}
        zero = Image.open(files[0])
        first = {name: measures(zero, box) for name, box in boxes.items()}
        resized = any(Image.open(f).size != zero.size for f in files)
        before: Image.Image | None = None
        for f in files:
            image = Image.open(f)
            kept = same(image, before) if before is not None else None
            for name, box in boxes.items():
                m = measures(image, box)
                if resized:
                    shown = [f"{m[0]:.0f}", f"{m[1]:.0f}", *(f"{100 * v / m[5]:.2f}%" for v in m[2:5])]
                else:
                    shown = [f"{x / y:.2f}" if y else "nan" for x, y in zip(m[:5], first[name][:5])]
                rows.append("\t".join([q["name"], f.stem, f"{image.width}x{image.height}", "" if kept is None else f"{kept:.3f}", name,
                                       *shown, f"{m[6]:.2f}"]))
            before = image
    text = "\n".join(rows) + "\n"
    if args.out:
        Path(args.out).write_text(text)
    print(text, end="")
    return 0


if __name__ == "__main__":
    sys.exit(main())
