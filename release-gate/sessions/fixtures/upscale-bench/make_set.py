"""Builds clean/ (the ground truth) and degraded/ (what an upscaler is given) from originals/, per sources.tsv.

clean: the crop, on white, Lanczos to `gt` px square. degraded: clean Lanczos to 225 px, then JPEG at `q`
(`sub` 444 or 420 chroma), as a web thumbnail like Paul's Groudon and Kyogre (225 px, q~74, 4:4:4).
    make_set.py [--fetch] [--clean-only]
--fetch downloads each original from its `url` first, checked against its `sha256`; --clean-only keeps degraded/ as
committed (a JPEG encoder may differ by a bit elsewhere).
"""

import argparse
import csv
import hashlib
import time
import urllib.request
from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
SMALL = 225

parser = argparse.ArgumentParser()
parser.add_argument("--fetch", action="store_true")
parser.add_argument("--clean-only", action="store_true")
args = parser.parse_args()
for folder in ("originals", "clean", "degraded"):
    (HERE / folder).mkdir(exist_ok=True)
for row in csv.DictReader(open(HERE / "sources.tsv"), delimiter="\t"):
    original = HERE / "originals" / row["file"]
    if args.fetch and not (original.exists() and hashlib.sha256(original.read_bytes()).hexdigest() == row["sha256"]):
        request = urllib.request.Request(row["url"], headers={"User-Agent": "upscale-bench/1 (cozy-creator release gate)"})
        for attempt in range(5):
            try:
                original.write_bytes(urllib.request.urlopen(request, timeout=120).read())
                break
            except OSError:
                time.sleep(10 * (attempt + 1))
    if hashlib.sha256(original.read_bytes()).hexdigest() != row["sha256"]:
        raise SystemExit(f"{row['file']}: not the sha256 sources.tsv names")
    image = Image.open(original).convert("RGBA")
    white = Image.new("RGBA", image.size, "white")
    white.alpha_composite(image)
    x, y, side, gt = (int(row[k]) for k in ("x", "y", "side", "gt"))
    clean = white.convert("RGB").crop((x, y, x + side, y + side)).resize((gt, gt), Image.Resampling.LANCZOS)
    clean.save(HERE / "clean" / f"{row['name']}.png")
    if not args.clean_only:
        small = clean.resize((SMALL, SMALL), Image.Resampling.LANCZOS)
        small.save(HERE / "degraded" / f"{row['name']}.jpg", quality=int(row["q"]), subsampling=0 if row["sub"] == "444" else 2)
    print(row["name"], clean.size, f"q{row['q']}", row["sub"])
