"""Each pair of pictures: PSNR (8-bit RGB), mean |difference| (0-255), DISTS (pyiqa, CPU), and a crop sheet to look at.

  python compare.py <pairs.tsv> <out dir>

pairs.tsv: name<TAB>reference png<TAB>candidate png per line. Writes <out>/metrics.tsv and, per pair, <out>/<name>.jpg:
reference | candidate | |difference| x4, a 384 px window from the centre and one from the most different 384 px block, each
at 2x (nearest)."""

import math
import os
import sys

import numpy as np
import pyiqa
import torch
from PIL import Image

pairs_path, out = sys.argv[1:3]
os.makedirs(out, exist_ok=True)
dists = pyiqa.create_metric("dists", device="cpu")
W = 384
rows = ["name\tpsnr_db\tmean_abs\tdists\tsize"]
for line in open(pairs_path):
    if not line.strip():
        continue
    name, a_path, b_path = line.rstrip("\n").split("\t")
    if not (os.path.exists(a_path) and os.path.exists(b_path)):
        rows.append(f"{name}\tmissing\t\t\t{a_path if not os.path.exists(a_path) else b_path}")
        continue
    a, b = (np.asarray(Image.open(p).convert("RGB"), dtype=np.float64) for p in (a_path, b_path))
    if a.shape != b.shape:
        rows.append(f"{name}\tsize differs\t\t\t{a.shape[1]}x{a.shape[0]} vs {b.shape[1]}x{b.shape[0]}")
        continue
    mse = float(((a - b) ** 2).mean())
    psnr = math.inf if mse == 0 else 10 * math.log10(255**2 / mse)
    tensors = [torch.from_numpy(x / 255.0).permute(2, 0, 1)[None].float() for x in (a, b)]
    with torch.no_grad():
        d = float(dists(tensors[1], tensors[0]))
    rows.append(f"{name}\t{psnr:.2f}\t{np.abs(a - b).mean():.3f}\t{d:.4f}\t{a.shape[1]}x{a.shape[0]}")
    h, w = a.shape[:2]
    diff = np.abs(a - b).mean(axis=2)
    blocks = diff[: h // W * W, : w // W * W].reshape(h // W, W, w // W, W).mean(axis=(1, 3))
    by, bx = np.unravel_index(int(blocks.argmax()), blocks.shape)
    windows = [((h - W) // 2, (w - W) // 2), (by * W, bx * W)]
    tiles = []
    for y, x in windows:
        crop = [img[y : y + W, x : x + W] for img in (a, b)]
        crop.append(np.clip(np.abs(crop[0] - crop[1]) * 4, 0, 255))
        tiles.append(np.concatenate(crop, axis=1))
    sheet = Image.fromarray(np.concatenate(tiles, axis=0).astype(np.uint8))
    sheet.resize((sheet.width * 2, sheet.height * 2), Image.NEAREST).save(os.path.join(out, name + ".jpg"), quality=92)
open(os.path.join(out, "metrics.tsv"), "w").write("\n".join(rows) + "\n")
print("\n".join(rows))
