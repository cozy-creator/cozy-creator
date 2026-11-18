"""Builds the edit bench from sources.tsv: originals/ (fetched, sha256-checked) and chain/<name>-0.png (its 4:5 crop at
1280x1600, a chain's generation 0).
    make_set.py [--from DIR] [--fetch]
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
#: Wikimedia's User-Agent policy: a name and a way to reach its maker.
AGENT = "cozy-qwen-edit-bench/1.0 (https://github.com/cozy-creator/cozy-creator) python-urllib"
CHAIN = (1280, 1600)

parser = argparse.ArgumentParser()
parser.add_argument("--from", dest="source", type=Path)
parser.add_argument("--fetch", action="store_true")
args = parser.parse_args()


def held(path: Path, sha256: str) -> bool:
    return path.exists() and hashlib.sha256(path.read_bytes()).hexdigest() == sha256


for folder in ("originals", "chain"):
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
                print(f"{row['file']}: {error} (attempt {attempt + 1}/4)", flush=True)
                time.sleep(15 * (attempt + 1))
    if not held(original, row["sha256"]):
        raise SystemExit(f"{row['file']}: no original with the sha256 sources.tsv names (--from, --fetch)")
    x, y, w, h = (int(v) for v in row["crop"].split(","))
    crop = Image.open(original).convert("RGB").crop((x, y, x + w, y + h))
    crop.resize(CHAIN, Image.Resampling.LANCZOS).save(HERE / "chain" / f"{row['name']}-0.png")
    print(row["name"], crop.size, "chain", CHAIN)
