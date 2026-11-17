"""Side-by-side sheets of every candidate's output for each input, to look at.

    sheets.py <runs dir> [--out DIR] [--only name,name]

Per input, <out>/<name>-full.jpg (each picture at 640 px) and <out>/<name>-crop.png (a 640 px window at the clean
image's size, where the clean image, else the Lanczos blow-up, has the most edges): clean, lanczos, then each
candidate in <runs dir>/<candidate>/<name>.png. Columns are labelled.
"""

import argparse
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).resolve().parent
SIDE, LABEL = 640, 36

parser = argparse.ArgumentParser()
parser.add_argument("runs", type=Path)
parser.add_argument("--out", type=Path)
parser.add_argument("--only", default="")
args = parser.parse_args()
out = args.out or args.runs / "sheets"
out.mkdir(parents=True, exist_ok=True)
font = ImageFont.load_default(size=26)
candidates = sorted(p for p in args.runs.iterdir() if p.is_dir() and p.name != out.name)
names = sorted({p.stem for c in candidates for p in c.glob("*.png")})
if args.only:
    names = [n for n in names if n in args.only.split(",")]


def window(image: Image.Image) -> tuple[int, int]:
    """The top-left of the SIDE window with the most edge energy."""
    gray = np.asarray(image.convert("L"), dtype=np.float32)
    energy = np.abs(np.diff(gray, axis=0))[:, :-1] + np.abs(np.diff(gray, axis=1))[:-1]
    step, best, at = SIDE // 4, -1.0, (0, 0)
    for y in range(0, energy.shape[0] - SIDE + 1, step):
        for x in range(0, energy.shape[1] - SIDE + 1, step):
            e = float(energy[y : y + SIDE, x : x + SIDE].sum())
            if e > best:
                best, at = e, (x, y)
    return at


def row(tiles: list[tuple[str, Image.Image]]) -> Image.Image:
    sheet = Image.new("RGB", (SIDE * len(tiles), SIDE + LABEL), "white")
    draw = ImageDraw.Draw(sheet)
    for i, (label, tile) in enumerate(tiles):
        sheet.paste(tile, (i * SIDE, LABEL))
        draw.text((i * SIDE + 8, 4), label, fill="black", font=font)
    return sheet


for name in names:
    clean_path = HERE / "clean" / f"{name}.png"
    degraded = next((p for p in (HERE / "degraded" / f"{name}.jpg", HERE / "paul" / f"{name}.jpg") if p.exists()), None)
    if degraded is None:
        continue
    pictures: list[tuple[str, Image.Image]] = []
    size = Image.open(clean_path).size if clean_path.exists() else (2048, 2048)
    if clean_path.exists():
        pictures.append(("clean", Image.open(clean_path).convert("RGB")))
    pictures.append(("lanczos", Image.open(degraded).convert("RGB").resize(size, Image.Resampling.LANCZOS)))
    for candidate in candidates:
        path = candidate / f"{name}.png"
        if path.exists():
            pictures.append((candidate.name, Image.open(path).convert("RGB").resize(size, Image.Resampling.LANCZOS)))
    x, y = window(pictures[0][1])
    row([(label, p.resize((SIDE, SIDE), Image.Resampling.LANCZOS)) for label, p in pictures]).save(out / f"{name}-full.jpg", quality=92)
    row([(label, p.crop((x, y, x + SIDE, y + SIDE))) for label, p in pictures]).save(out / f"{name}-crop.png")
    print(name, len(pictures), "columns, crop at", (x, y))
