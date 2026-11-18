"""Builds the edit bench from sources.tsv: originals/ (from DIR, sha256-checked) and chain/<name>-0.png (its 4:5 crop at
1280x1600, a chain's generation 0).
    make_set.py --from DIR
DIR holds each `file`: the gate's checkout of the private benchmarks repo's edit/originals. Its `url` is where the photo came
from and its licence, never fetched by a gate, so no row depends on a third-party site.
"""

import argparse
import csv
import hashlib
import shutil
from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
CHAIN = (1280, 1600)

parser = argparse.ArgumentParser()
parser.add_argument("--from", dest="source", type=Path, required=True)
args = parser.parse_args()


def held(path: Path, sha256: str) -> bool:
    return path.exists() and hashlib.sha256(path.read_bytes()).hexdigest() == sha256


for folder in ("originals", "chain"):
    (HERE / folder).mkdir(exist_ok=True)
for row in csv.DictReader(open(HERE / "sources.tsv"), delimiter="\t"):
    original = HERE / "originals" / row["file"]
    if not held(original, row["sha256"]) and held(args.source / row["file"], row["sha256"]):
        shutil.copyfile(args.source / row["file"], original)
    if not held(original, row["sha256"]):
        raise SystemExit(f"{row['file']}: not in {args.source} with the sha256 sources.tsv names")
    x, y, w, h = (int(v) for v in row["crop"].split(","))
    crop = Image.open(original).convert("RGB").crop((x, y, x + w, y + h))
    crop.resize(CHAIN, Image.Resampling.LANCZOS).save(HERE / "chain" / f"{row['name']}-0.png")
    print(row["name"], crop.size, "chain", CHAIN)
