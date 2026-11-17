"""Scores upscaler outputs against the clean originals.

    metrics.py <runs dir> [--out scores.tsv] [--images name,name]

<runs dir>/<candidate>/<name>.png (or .jpg/.webp) is one candidate's output for degraded/<name>.jpg; a name with a
clean/<name>.png gets full-reference scores, any other (paul/'s groudon, kyogre) no-reference only.
- Full reference, at the clean image's size (the output Lanczos-resized to it): PSNR (RGB), LPIPS (AlexNet) and DISTS,
  lower better for both.
- No reference, on the output Lanczos-resized to 1024 px on its long side (CPU-friendly): MUSIQ (KonIQ) and CLIP-IQA,
  higher better.
- Framing: the affine that maps the output onto the clean picture (else the input's Lanczos blow-up), by OpenCV's ECC at
  512 px: `scale` (1 keeps the framing; 1.05 means the output is zoomed in 5%) and `shift`, its offset at the centre as a
  share of the side (0.01 = 1%).
`lanczos` (degraded/ and paul/ Lanczos to the clean size, else 2048²) is scored too, as the floor. Prints per-candidate means: art (kind art
or painting in sources.tsv), photo, and all.
"""

import argparse
import csv
import math
from importlib import import_module
from pathlib import Path

import numpy as np
import torch
from PIL import Image

# OpenCV and pyiqa ship no types (mypy has none of them), so they are imported by name.
cv2 = import_module("cv2")
pyiqa = import_module("pyiqa")

HERE = Path(__file__).resolve().parent
NR_SIDE = 1024

parser = argparse.ArgumentParser()
parser.add_argument("runs", type=Path)
parser.add_argument("--out", type=Path)
parser.add_argument("--images", default="", help="only these inputs (default: every one)")
args = parser.parse_args()
device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
kinds = {r["name"]: r["kind"] for r in csv.DictReader(open(HERE / "sources.tsv"), delimiter="\t")}
lpips, dists = pyiqa.create_metric("lpips", device=device), pyiqa.create_metric("dists", device=device)
musiq, clipiqa = pyiqa.create_metric("musiq", device=device), pyiqa.create_metric("clipiqa", device=device)


def tensor(image: Image.Image) -> torch.Tensor:
    return torch.from_numpy(np.asarray(image.convert("RGB"), dtype=np.float32) / 255).permute(2, 0, 1)[None].to(device)


def psnr(a: Image.Image, b: Image.Image) -> float:
    mse = float(np.mean((np.asarray(a, dtype=np.float64) - np.asarray(b, dtype=np.float64)) ** 2))
    return 10 * math.log10(255**2 / mse) if mse else 99.0


def framing(output: Image.Image, reference: Image.Image) -> dict[str, float]:
    """The output's zoom and offset against the reference: ECC's affine of the reference onto the output at 512 px."""
    side = 512
    a = np.asarray(reference.convert("L").resize((side, side), Image.Resampling.LANCZOS), dtype=np.float32) / 255
    b = np.asarray(output.convert("L").resize((side, side), Image.Resampling.LANCZOS), dtype=np.float32) / 255
    warp = np.eye(2, 3, dtype=np.float32)
    try:
        _, warp = cv2.findTransformECC(a, b, warp, cv2.MOTION_AFFINE, (cv2.TERM_CRITERIA_EPS | cv2.TERM_CRITERIA_COUNT, 200, 1e-6), None, 5)
    except cv2.error:
        return {"scale": float("nan"), "shift": float("nan")}
    # warp maps reference coordinates to output coordinates: its linear part's scale is the output's zoom.
    scale = math.sqrt(abs(float(np.linalg.det(warp[:, :2]))))
    centre = warp[:, :2] @ np.array([side / 2, side / 2]) + warp[:, 2] - side / 2
    return {"scale": scale, "shift": float(np.hypot(*centre)) / side}


def score(output: Image.Image, name: str) -> dict[str, float]:
    output = output.convert("RGB")
    row: dict[str, float] = {}
    clean_path = HERE / "clean" / f"{name}.png"
    if clean_path.exists():
        clean = Image.open(clean_path).convert("RGB")
        fitted = output.resize(clean.size, Image.Resampling.LANCZOS)
        a, b = tensor(fitted), tensor(clean)
        row |= {"psnr": psnr(fitted, clean), "lpips": float(lpips(a, b)), "dists": float(dists(a, b))}
        row |= framing(output, clean)
    else:
        source = next(p for p in (HERE / "degraded" / f"{name}.jpg", HERE / "paul" / f"{name}.jpg") if p.exists())
        row |= framing(output, Image.open(source).resize(output.size, Image.Resampling.LANCZOS))
    s = NR_SIDE / max(output.size)
    small = output.resize((round(output.width * s), round(output.height * s)), Image.Resampling.LANCZOS)
    t = tensor(small)
    row |= {"musiq": float(musiq(t)), "clipiqa": float(clipiqa(t))}
    return row


def outputs(folder: Path) -> dict[str, Path]:
    return {p.stem: p for p in sorted(folder.iterdir()) if p.suffix.lower() in (".png", ".jpg", ".jpeg", ".webp")}


rows: list[dict[str, str | float]] = []
candidates: dict[str, dict[str, Path]] = {"lanczos": {}}
for name in kinds:
    candidates["lanczos"][name] = HERE / "degraded" / f"{name}.jpg"
for path in sorted((HERE / "paul").glob("*.jpg")):
    candidates["lanczos"][path.stem] = path
for folder in sorted(p for p in args.runs.iterdir() if p.is_dir() and p.name != "sheets"):
    candidates[folder.name] = outputs(folder)
only = set(filter(None, args.images.split(",")))
for candidate, files in candidates.items():
    for name, path in files.items():
        if only and name not in only:
            continue
        image: Image.Image = Image.open(path)
        if candidate == "lanczos":
            clean = HERE / "clean" / f"{name}.png"
            image = image.resize(Image.open(clean).size if clean.exists() else (2048, 2048), Image.Resampling.LANCZOS)
        row: dict[str, str | float] = {"candidate": candidate, "image": name, "kind": kinds.get(name, "paul"), "size": f"{image.width}x{image.height}"}
        row |= {k: round(v, 4) for k, v in score(image, name).items()}
        rows.append(row)
        print("\t".join(str(v) for v in row.values()), flush=True)

columns = ["candidate", "image", "kind", "size", "psnr", "lpips", "dists", "musiq", "clipiqa", "scale", "shift"]
out = args.out or args.runs / "scores.tsv"
with open(out, "w") as f:
    writer = csv.DictWriter(f, columns, delimiter="\t", restval="")
    writer.writeheader()
    writer.writerows(rows)
print(f"\n{'candidate':28} {'set':6} {'n':>2} {'PSNR':>6} {'LPIPS':>6} {'DISTS':>6} {'MUSIQ':>6} {'CLIPIQA':>7} {'|zoom|':>6} {'shift':>6}")
for candidate in candidates:
    for label, keep in (("art", ("art", "painting")), ("photo", ("photo",)), ("all", ("art", "painting", "photo")), ("paul", ("paul",))):
        mine = [r for r in rows if r["candidate"] == candidate and r["kind"] in keep]
        if not mine:
            continue
        mean = {k: sum(float(r[k]) for r in mine) / len(mine) if all(k in r for r in mine) else float("nan")
                for k in ("psnr", "lpips", "dists", "musiq", "clipiqa", "shift")}
        mean["zoom"] = sum(abs(float(r["scale"]) - 1) for r in mine) / len(mine)
        print(f"{candidate:28} {label:6} {len(mine):>2} {mean['psnr']:6.2f} {mean['lpips']:6.3f} {mean['dists']:6.3f} "
              f"{mean['musiq']:6.2f} {mean['clipiqa']:7.3f} {mean['zoom']:6.3f} {mean['shift']:6.3f}")
