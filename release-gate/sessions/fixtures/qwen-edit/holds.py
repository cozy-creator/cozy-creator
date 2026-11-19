"""Holds a chain of image edits to the photo it started from, and to what its edits drew (suite edit, se-153).

    holds.py chain <bench name> <bar> <generation 0> <generation N>
    holds.py crisp <bench name> <bar> <generation 1> <generation N>
    holds.py lands <bench name> <bar> <generation 0> <generation 1> ... <generation N>

chain: the photo's `still` box (sources.tsv: no edit of the chain touches it) of generation N, aligned to generation 0 by
ECC, held to generation 0: DISTS at most `bar`.
crisp: the `made` box (what the first edit drew) of generation N, its fine detail (the variance of luminance's Laplacian)
at most `bar` times generation 1's: a region re-rendered by later edits sharpens every time.
lands: each edit's own boxes (sources.tsv `lands`: one group per edit in the chain's order, `+` between a group's boxes) in
generation k, aligned to generation k-1 by ECC over the `still` box: at least `bar` of their pixels moved by more than
LANDED_DELTA_E, for every edit. An edit that left its boxes as they were (a paste-back keeping too much) did not land.
A picture that cannot be aligned is a FAIL that says so, never a number.
One line, PASS or FAIL with the numbers; exits 1 on FAIL.
"""

import csv
import sys
from importlib import import_module
from pathlib import Path
from typing import NoReturn

import numpy as np
import numpy.typing as npt
from PIL import Image

# OpenCV ships no types (mypy has none of it), so it is imported by name; pyiqa (and torch) only where DISTS is taken.
cv2 = import_module("cv2")
#: A box's pixel moved by more than this (CIE76 on Lab, after a 1 px blur) is the edit's (keep.py's CHANGED_DELTA_E).
LANDED_DELTA_E = 10.0

HERE = Path(__file__).resolve().parent
sources = {r["name"]: r for r in csv.DictReader(open(HERE / "sources.tsv"), delimiter="\t")}


def boxes(text: str) -> list[tuple[float, ...]]:
    return [tuple(float(v) for v in b.split(",")) for b in text.split(";") if b]


def groups(text: str) -> list[list[tuple[float, ...]]]:
    return [boxes(g.replace("+", ";")) for g in text.split(";") if g]


def rgb(path: Path) -> Image.Image:
    image = Image.open(path)
    if "A" in image.getbands():
        white = Image.new("RGBA", image.size, "white")
        white.alpha_composite(image.convert("RGBA"))
        return white.convert("RGB")
    return image.convert("RGB")


def aligned(output: Image.Image, truth: Image.Image, regions: list[tuple[float, ...]]) -> Image.Image:
    """The output warped onto the truth by ECC (affine, over the regions, at 1024 px on the long side); cv2.error when ECC
    finds no alignment."""
    w, h = truth.size
    s = 1024 / max(w, h)
    size = (round(w * s), round(h * s))
    a = np.asarray(truth.convert("L").resize(size, Image.Resampling.LANCZOS), dtype=np.float32) / 255
    b = np.asarray(output.convert("L").resize(size, Image.Resampling.LANCZOS), dtype=np.float32) / 255
    mask = np.zeros(a.shape, np.uint8)
    for x0, y0, x1, y1 in regions:
        mask[round(y0 * size[1]) : round(y1 * size[1]), round(x0 * size[0]) : round(x1 * size[0])] = 1
    warp = np.eye(2, 3, dtype=np.float32)
    _, warp = cv2.findTransformECC(a, b, warp, cv2.MOTION_AFFINE, (cv2.TERM_CRITERIA_EPS | cv2.TERM_CRITERIA_COUNT, 200, 1e-6), mask, 5)
    warp[:, 2] /= s
    return Image.fromarray(cv2.warpAffine(np.asarray(output), warp, (w, h), flags=cv2.INTER_CUBIC | cv2.WARP_INVERSE_MAP, borderMode=cv2.BORDER_REPLICATE))


def cropped(image: Image.Image, box: tuple[float, ...]) -> Image.Image:
    w, h = image.size
    return image.crop((round(box[0] * w), round(box[1] * h), round(box[2] * w), round(box[3] * h)))


def held(a: Image.Image, b: Image.Image, box: tuple[float, ...]) -> float:
    import torch

    pyiqa = import_module("pyiqa")

    def tensor(image: Image.Image) -> torch.Tensor:
        return torch.from_numpy(np.asarray(cropped(image, box), dtype=np.float32) / 255).permute(2, 0, 1)[None]

    return float(pyiqa.create_metric("dists", device="cpu")(tensor(a), tensor(b)))


def detail(image: Image.Image, box: tuple[float, ...]) -> float:
    y = np.asarray(cropped(image, box).convert("L"), dtype=np.float64)
    return float((-4 * y[1:-1, 1:-1] + y[:-2, 1:-1] + y[2:, 1:-1] + y[1:-1, :-2] + y[1:-1, 2:]).var())


def moved(after: Image.Image, before: Image.Image, regions: list[tuple[float, ...]]) -> float:
    """The share of the regions' pixels whose colour moved by more than LANDED_DELTA_E from before to after."""
    def lab(image: Image.Image, box: tuple[float, ...]) -> npt.NDArray[np.float32]:
        x = cv2.GaussianBlur(np.asarray(cropped(image, box), dtype=np.float32) / 255, (0, 0), 1.0)
        return np.asarray(cv2.cvtColor(x, cv2.COLOR_RGB2Lab), dtype=np.float32)

    shifted = [np.linalg.norm(lab(after, box) - lab(before, box), axis=2) > LANDED_DELTA_E for box in regions]
    return float(sum(s.sum() for s in shifted) / sum(s.size for s in shifted))


def unaligned(picture: Path, against: str, error: Exception) -> NoReturn:
    print(f"FAIL {name} {picture.stem}: could not align it to {against} ({' '.join(str(error).split())[:200]})")
    sys.exit(1)


mode, name, bar = sys.argv[1], sys.argv[2], float(sys.argv[3])
pictures = [Path(p) for p in sys.argv[4:]]
earlier, last = pictures[0], pictures[-1]
out = rgb(last)
if mode == "chain":
    still = boxes(sources[name]["still"])
    truth = rgb(earlier).resize(out.size, Image.Resampling.LANCZOS)
    try:
        moved_onto = aligned(out, truth, still)
    except cv2.error as error:
        unaligned(last, "generation 0", error)
    d = held(moved_onto, truth, still[0])
    ok = d <= bar
    print(f"{'PASS' if ok else 'FAIL'} {name} {last.stem} against generation 0: still box DISTS {d:.4f} (the bar {bar})")
elif mode == "crisp":
    made = boxes(sources[name]["made"])[0]
    ratio = detail(out, made) / detail(rgb(earlier), made)
    ok = ratio <= bar
    print(f"{'PASS' if ok else 'FAIL'} {name} {last.stem} against generation 1: the box its first edit drew has {ratio:.2f}x "
          f"its fine detail (the bar {bar}x)")
elif mode == "lands":
    edits, still = groups(sources[name]["lands"]), boxes(sources[name]["still"])
    if len(edits) != len(pictures) - 1:
        raise SystemExit(f"{name}: {len(pictures) - 1} edits, and sources.tsv `lands` has boxes for {len(edits)}")
    shares = []
    for k, regions in enumerate(edits, 1):
        after = rgb(pictures[k])
        before = rgb(pictures[k - 1]).resize(after.size, Image.Resampling.LANCZOS)
        try:
            shares.append(moved(aligned(after, before, still), before, regions))
        except cv2.error as error:
            unaligned(pictures[k], f"generation {k - 1}", error)
    ok = min(shares) >= bar
    print(f"{'PASS' if ok else 'FAIL'} {name} {last.stem}: how much of its own boxes each edit changed, "
          + ", ".join(f"g{k} {s:.1%}" for k, s in enumerate(shares, 1)) + f" (the bar {bar:.0%})")
else:
    raise SystemExit(__doc__)
sys.exit(0 if ok else 1)
