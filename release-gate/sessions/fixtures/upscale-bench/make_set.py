"""Builds clean/ (the ground truth) and degraded/ (what an upscaler is given) from originals/, per sources.tsv.

clean: the crop, on white, Lanczos to `gt` px square. degraded: clean Lanczos to 225 px, then JPEG at `q`
(`sub` 444 or 420 chroma), as a web thumbnail like Paul's Groudon and Kyogre (225 px, q~74, 4:4:4).
    make_set.py [--from DIR] [--fetch] [--clean-only]
Each original is checked against its `sha256`. A missing one is copied from DIR (the private benchmarks repo's
upscale/originals) or, with --fetch, downloaded from its `url`. --clean-only keeps degraded/ as committed (a JPEG
encoder elsewhere may differ by a bit).
"""

import argparse
import csv
import hashlib
import shutil
import time
import urllib.error
import urllib.request
from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
SMALL = 225
#: Wikimedia's User-Agent policy: a name and a way to reach its maker.
AGENT = "cozy-upscale-bench/1.0 (https://github.com/cozy-creator/cozy-creator) python-urllib"

parser = argparse.ArgumentParser()
parser.add_argument("--from", dest="source", type=Path)
parser.add_argument("--fetch", action="store_true")
parser.add_argument("--clean-only", action="store_true")
args = parser.parse_args()


def held(path: Path, sha256: str) -> bool:
    return path.exists() and hashlib.sha256(path.read_bytes()).hexdigest() == sha256


for folder in ("originals", "clean", "degraded"):
    (HERE / folder).mkdir(exist_ok=True)
for row in csv.DictReader(open(HERE / "sources.tsv"), delimiter="\t"):
    original = HERE / "originals" / row["file"]
    if not held(original, row["sha256"]) and args.source and held(args.source / row["file"], row["sha256"]):
        shutil.copyfile(args.source / row["file"], original)
    if not held(original, row["sha256"]) and args.fetch:
        request = urllib.request.Request(row["url"], headers={"User-Agent": AGENT})
        for attempt in range(4):
            try:
                original.write_bytes(urllib.request.urlopen(request, timeout=120).read())
                break
            except (urllib.error.URLError, TimeoutError) as error:
                print(f"{row['file']}: {row['url']}: {error} (attempt {attempt + 1}/4)", flush=True)
                time.sleep(15 * (attempt + 1))
    if not held(original, row["sha256"]):
        raise SystemExit(f"{row['file']}: no original with the sha256 sources.tsv names (--from, --fetch)")
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
