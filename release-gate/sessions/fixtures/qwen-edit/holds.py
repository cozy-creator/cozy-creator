"""Holds a chain of image edits to the photo it started from, and to what its edits drew (suite edit, se-153).

    holds.py chain <bench name> <bar> <generation 0> <generation N>
    holds.py crisp <bench name> <bar> <generation 1> <generation N>

chain: the photo's `still` box (sources.tsv: no edit of the chain touches it) of generation N, aligned to generation 0 by
ECC, held to generation 0: DISTS at most `bar`.
crisp: the `made` box (what the first edit drew) of generation N, its fine detail (the variance of luminance's Laplacian)
at most `bar` times generation 1's: a region re-rendered by later edits sharpens every time.
One line, PASS or FAIL with the numbers; exits 1 on FAIL.
"""

import csv
import sys
from importlib import import_module
from pathlib import Path

import numpy as np
import torch
from PIL import Image

# OpenCV and pyiqa ship no types (mypy has none of them), so they are imported by name.
cv2 = import_module("cv2")
pyiqa = import_module("pyiqa")

HERE = Path(__file__).resolve().parent
sources = {r["name"]: r for r in csv.DictReader(open(HERE / "sources.tsv"), delimiter="\t")}


def boxes(text: str) -> list[tuple[float, ...]]:
    return [tuple(float(v) for v in b.split(",")) for b in text.split(";") if b]


def rgb(path: Path) -> Image.Image:
    image = Image.open(path)
    if "A" in image.getbands():
        white = Image.new("RGBA", image.size, "white")
        white.alpha_composite(image.convert("RGBA"))
        return white.convert("RGB")
    return image.convert("RGB")


def aligned(output: Image.Image, truth: Image.Image, regions: list[tuple[float, ...]]) -> Image.Image:
    """The output warped onto the truth by ECC (affine, over the regions, at 1024 px on the long side)."""
    w, h = truth.size
    s = 1024 / max(w, h)
    size = (round(w * s), round(h * s))
    a = np.asarray(truth.convert("L").resize(size, Image.Resampling.LANCZOS), dtype=np.float32) / 255
    b = np.asarray(output.convert("L").resize(size, Image.Resampling.LANCZOS), dtype=np.float32) / 255
    mask = np.zeros(a.shape, np.uint8)
    for x0, y0, x1, y1 in regions:
        mask[round(y0 * size[1]) : round(y1 * size[1]), round(x0 * size[0]) : round(x1 * size[0])] = 1
    warp = np.eye(2, 3, dtype=np.float32)
    try:
        _, warp = cv2.findTransformECC(a, b, warp, cv2.MOTION_AFFINE, (cv2.TERM_CRITERIA_EPS | cv2.TERM_CRITERIA_COUNT, 200, 1e-6), mask, 5)
    except cv2.error:
        return output
    warp[:, 2] /= s
    return Image.fromarray(cv2.warpAffine(np.asarray(output), warp, (w, h), flags=cv2.INTER_CUBIC | cv2.WARP_INVERSE_MAP, borderMode=cv2.BORDER_REPLICATE))


def cropped(image: Image.Image, box: tuple[float, ...]) -> Image.Image:
    w, h = image.size
    return image.crop((round(box[0] * w), round(box[1] * h), round(box[2] * w), round(box[3] * h)))


def held(a: Image.Image, b: Image.Image, box: tuple[float, ...]) -> float:
    def tensor(image: Image.Image) -> torch.Tensor:
        return torch.from_numpy(np.asarray(cropped(image, box), dtype=np.float32) / 255).permute(2, 0, 1)[None]

    return float(pyiqa.create_metric("dists", device="cpu")(tensor(a), tensor(b)))


def detail(image: Image.Image, box: tuple[float, ...]) -> float:
    y = np.asarray(cropped(image, box).convert("L"), dtype=np.float64)
    return float((-4 * y[1:-1, 1:-1] + y[:-2, 1:-1] + y[2:, 1:-1] + y[1:-1, :-2] + y[1:-1, 2:]).var())


mode, name, bar, earlier, last = sys.argv[1], sys.argv[2], float(sys.argv[3]), Path(sys.argv[4]), Path(sys.argv[5])
out = rgb(last)
if mode == "chain":
    still = boxes(sources[name]["still"])
    truth = rgb(earlier).resize(out.size, Image.Resampling.LANCZOS)
    d = held(aligned(out, truth, still), truth, still[0])
    ok = d <= bar
    print(f"{'PASS' if ok else 'FAIL'} {name} {last.stem} against generation 0: still box DISTS {d:.4f} (the bar {bar})")
else:
    made = boxes(sources[name]["made"])[0]
    ratio = detail(out, made) / detail(rgb(earlier), made)
    ok = ratio <= bar
    print(f"{'PASS' if ok else 'FAIL'} {name} {last.stem} against generation 1: the box its first edit drew has {ratio:.2f}x "
          f"its fine detail (the bar {bar}x)")
sys.exit(0 if ok else 1)
