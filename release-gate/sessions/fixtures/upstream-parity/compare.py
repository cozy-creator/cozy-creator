"""se-158: pairs of pictures scored (PSNR in dB, higher closer; DISTS, lower closer) and cropped side by side.

    compare.py <pairs.tsv> --out <scores.tsv> --crops <dir>

Each pairs.tsv row is `name  a.png  b.png  meaning`. A pair of different sizes is a failure, not a resize. Each crop
is the same 384² window of both pictures (the centre, and the busiest window by edge energy), shown at 2x, a left
and b right.
"""

import argparse
import csv
import math
from pathlib import Path

import numpy as np
import pyiqa
import torch
from PIL import Image, ImageFilter

parser = argparse.ArgumentParser()
parser.add_argument("pairs", type=Path)
parser.add_argument("--out", type=Path, required=True)
parser.add_argument("--crops", type=Path, required=True)
args = parser.parse_args()
args.crops.mkdir(parents=True, exist_ok=True)
dists = pyiqa.create_metric("dists", device="cpu")
SIDE = 384


def psnr(a: np.ndarray, b: np.ndarray) -> float:
    mse = float(np.mean((a.astype(np.float64) - b.astype(np.float64)) ** 2))
    return 99.0 if mse == 0 else 10 * math.log10(255**2 / mse)


def busiest(image: Image.Image) -> tuple[int, int]:
    edges = np.asarray(image.convert("L").filter(ImageFilter.FIND_EDGES), dtype=np.float64)
    best, at = -1.0, (0, 0)
    for y in range(0, image.height - SIDE + 1, SIDE // 4):
        for x in range(0, image.width - SIDE + 1, SIDE // 4):
            e = float(edges[y : y + SIDE, x : x + SIDE].sum())
            if e > best:
                best, at = e, (x, y)
    return at


rows = []
for line in args.pairs.read_text().splitlines():
    if not line.strip():
        continue
    name, a_path, b_path, meaning = line.split("\t")
    if not (Path(a_path).exists() and Path(b_path).exists()):
        rows.append({"pair": name, "a": a_path, "b": b_path, "meaning": meaning, "psnr": "missing", "dists": "missing"})
        print(f"{name:34} MISSING {a_path if not Path(a_path).exists() else b_path}", flush=True)
        continue
    a, b = Image.open(a_path).convert("RGB"), Image.open(b_path).convert("RGB")
    if a.size != b.size:
        rows.append({"pair": name, "a": a_path, "b": b_path, "meaning": meaning, "psnr": f"size {a.size} vs {b.size}", "dists": ""})
        print(f"{name:34} SIZE {a.size} vs {b.size}", flush=True)
        continue
    ta, tb = (torch.from_numpy(np.asarray(x, dtype=np.float32) / 255).permute(2, 0, 1)[None] for x in (a, b))
    with torch.no_grad():
        d = float(dists(ta, tb))
    p = psnr(np.asarray(a), np.asarray(b))
    rows.append({"pair": name, "a": a_path, "b": b_path, "meaning": meaning, "psnr": round(p, 2), "dists": round(d, 4)})
    print(f"{name:34} PSNR {p:6.2f} dB  DISTS {d:.4f}  {a.width}x{a.height}  {meaning}", flush=True)
    for label, (x, y) in (("centre", ((a.width - SIDE) // 2, (a.height - SIDE) // 2)), ("busiest", busiest(a))):
        box = (x, y, x + SIDE, y + SIDE)
        sheet = Image.new("RGB", (SIDE * 4 + 8, SIDE * 2), "white")
        sheet.paste(a.crop(box).resize((SIDE * 2, SIDE * 2), Image.Resampling.NEAREST), (0, 0))
        sheet.paste(b.crop(box).resize((SIDE * 2, SIDE * 2), Image.Resampling.NEAREST), (SIDE * 2 + 8, 0))
        sheet.save(args.crops / f"{name}-{label}.png")
with open(args.out, "w") as f:
    writer = csv.DictWriter(f, ["pair", "psnr", "dists", "meaning", "a", "b"], delimiter="\t")
    writer.writeheader()
    writer.writerows(rows)
