"""Scores qwen-edit's outputs (session qwen-edit.sh) and lays out crops for the eye.

    measure.py --plan plan.json --work DIR [--records DIR] --out DIR [--selftest]

DIR/runs/<arm>/<case>--<mp>.<ext>: one edit of inputs/<input>.jpg. For a case of kind `keep` or `gray`, each kept box
(sources.tsv `keep`, or the case's `boxes`) of the output, aligned to the clean photo by ECC over the boxes, is held to the
clean photo Lanczos-resized to the output's size: DISTS and LPIPS (lower better), `hf` its Laplacian variance over the
clean photo's (1 is the photo's detail, under 1 softer), `de` the mean Lab colour shift; `gray` compares luminance only.
`input` rows score the input itself (the JPEG Lanczos to the output's size): the detail the edit was given. Every output
also gets `hf_all`, its whole-picture Laplacian variance at 1024 px on the long side. `scale` and `shift` (px at the output's
size) are the ECC alignment: the output's zoom and offset against the photo.
DIR/chains/<chain>/<arm>/g<k>.<ext>: generation k of a chain from chain/<start>-0.png; its `still` box (sources.tsv, never
edited) held to generation 0 the same way.
Writes scores.tsv, chains.tsv, summary.md and sheets/ (crops at 100%, chains at 200%).
"""

import argparse
import csv
import json
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
parser = argparse.ArgumentParser()
parser.add_argument("--plan", type=Path, required=True)
parser.add_argument("--work", type=Path, required=True)
parser.add_argument("--records", type=Path)
parser.add_argument("--out", type=Path, required=True)
parser.add_argument("--selftest", action="store_true")
args = parser.parse_args()
plan = json.loads(args.plan.read_text())
sources = {r["name"]: r for r in csv.DictReader(open(HERE / "sources.tsv"), delimiter="\t")}
cases = {c["name"]: c for group in plan["cases"].values() for c in group}
lpips, dists = pyiqa.create_metric("lpips", device="cpu"), pyiqa.create_metric("dists", device="cpu")
args.out.mkdir(parents=True, exist_ok=True)
(args.out / "sheets").mkdir(exist_ok=True)


def boxes(text: str) -> list[tuple[float, ...]]:
    return [tuple(float(v) for v in b.split(",")) for b in text.split(";") if b]


def rgb(path: Path) -> Image.Image:
    image = Image.open(path)
    if image.mode in ("RGBA", "LA", "P"):
        white = Image.new("RGBA", image.size, "white")
        white.alpha_composite(image.convert("RGBA"))
        return white.convert("RGB")
    return image.convert("RGB")


def tensor(image: Image.Image) -> torch.Tensor:
    return torch.from_numpy(np.asarray(image, dtype=np.float32) / 255).permute(2, 0, 1)[None]


def lapvar(image: Image.Image) -> float:
    return float(cv2.Laplacian(np.asarray(image.convert("L"), dtype=np.float32), cv2.CV_32F).var())


def lab_mean(image: Image.Image) -> np.ndarray:
    return cv2.cvtColor(np.asarray(image, dtype=np.float32) / 255, cv2.COLOR_RGB2LAB).reshape(-1, 3).mean(0)


def align(output: Image.Image, truth: Image.Image, regions: list[tuple[float, ...]]) -> tuple[Image.Image, float, float]:
    """The output warped onto the truth by ECC (affine, over the regions, at 1024 px), its zoom and its offset in px."""
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
        return output, float("nan"), float("nan")
    scale = math.sqrt(abs(float(np.linalg.det(warp[:, :2]))))
    centre = warp[:, :2] @ np.array([size[0] / 2, size[1] / 2]) + warp[:, 2] - np.array(size) / 2
    full = warp.copy()
    full[:, 2] /= s
    moved = cv2.warpAffine(np.asarray(output), full, (w, h), flags=cv2.INTER_CUBIC | cv2.WARP_INVERSE_MAP, borderMode=cv2.BORDER_REPLICATE)
    return Image.fromarray(moved), scale, float(np.hypot(*centre)) / s


def crop(image: Image.Image, box: tuple[float, ...]) -> Image.Image:
    w, h = image.size
    return image.crop((round(box[0] * w), round(box[1] * h), round(box[2] * w), round(box[3] * h)))


def held(out: Image.Image, truth: Image.Image, box: tuple[float, ...], gray: bool) -> dict[str, float]:
    a, b = crop(out, box), crop(truth, box)
    if gray:
        a, b = a.convert("L").convert("RGB"), b.convert("L").convert("RGB")
    ta, tb = tensor(a), tensor(b)
    shift = lab_mean(a) - lab_mean(b)
    return {"dists": float(dists(ta, tb)), "lpips": float(lpips(ta, tb)), "hf": lapvar(a) / max(lapvar(b), 1e-6),
            "de": float(abs(shift[0]) if gray else np.linalg.norm(shift))}


def timing(name: str) -> dict[str, float]:
    if not args.records or not (args.records / f"{name}.json").exists():
        return {}
    lines = [line for line in (args.records / f"{name}.json").read_text().splitlines() if line.startswith("{")]
    if not lines:
        return {}
    record = json.loads(lines[-1])
    stages = record.get("measurements", {}).get("attribution", {}).get("stages", {})
    t = {"exec_s": (record.get("execution_ms") or 0) / 1000}
    for stage, key in (("generating image", "gen_s"), ("encoding reference images", "ref_s"), ("encoding prompt", "prompt_s"),
                       ("keeping what stayed", "keep_s")):
        if stage in stages:
            t[key] = stages[stage]["total_ms"] / 1000
    return t


def fmt(v: object) -> str:
    return f"{v:.4f}" if isinstance(v, float) else str(v)


def sheet(tiles: list[tuple[str, Image.Image]], path: Path, zoom: int = 1) -> None:
    tiles = [(label, t.resize((t.width * zoom, t.height * zoom), Image.Resampling.NEAREST)) for label, t in tiles]
    h = max(t.height for _, t in tiles)
    canvas = Image.new("RGB", (sum(t.width for _, t in tiles) + 8 * len(tiles), h + 20), "white")
    x = 0
    draw = import_module("PIL.ImageDraw").Draw(canvas)
    for label, t in tiles:
        canvas.paste(t, (x, 20))
        draw.text((x + 4, 4), label, fill="black")
        x += t.width + 8
    canvas.save(path, quality=95)


if args.selftest:
    for name, row in sources.items():
        clean = rgb(HERE / "clean" / f"{name}.png").resize((1280, 1600), Image.Resampling.LANCZOS)
        given = rgb(HERE / "inputs" / f"{name}.jpg").resize(clean.size, Image.Resampling.LANCZOS)
        moved, scale, shift = align(given, clean, boxes(row["keep"]))
        print(name, {k: round(v, 4) for k, v in held(moved, clean, boxes(row["keep"])[0], False).items()}, f"scale {scale:.4f} shift {shift:.2f}")
    raise SystemExit(0)

# Single edits.
rows: list[dict[str, object]] = []
runs = args.work / "runs"
arms = [a["name"] for a in plan["arms"]]
for arm in arms:
    for path in sorted((runs / arm).glob("*.*")) if (runs / arm).exists() else []:
        name, mp = path.stem.rsplit("--", 1)
        case = cases[name]
        source = sources[case["input"]]
        out = rgb(path)
        base = {"arm": arm, "case": name, "mp": mp, "kind": case["kind"], "size": f"{out.width}x{out.height}",
                "hf_all": lapvar(out.resize((round(out.width * 1024 / max(out.size)), round(out.height * 1024 / max(out.size))), Image.Resampling.LANCZOS))}
        base |= timing(f"{arm}--{name}--{mp}")
        if case["kind"] == "none":
            rows.append(base)
            continue
        regions = boxes(case.get("boxes") or source["keep"])
        clean = rgb(HERE / "clean" / f"{case['input']}.png").resize(out.size, Image.Resampling.LANCZOS)
        given = rgb(HERE / "inputs" / f"{case['input']}.jpg").resize(out.size, Image.Resampling.LANCZOS)
        moved, scale, shift = align(out, clean, regions)
        for n, box in enumerate(regions):
            rows.append(base | {"box": n, "scale": scale, "shift": shift} | held(moved, clean, box, case["kind"] == "gray"))
            if arm == arms[0]:
                rows.append({"arm": "input", "case": name, "mp": mp, "kind": case["kind"], "box": n} | held(given, clean, box, case["kind"] == "gray"))

columns = ["arm", "case", "mp", "kind", "box", "size", "dists", "lpips", "hf", "de", "scale", "shift", "hf_all", "exec_s", "gen_s", "ref_s", "prompt_s", "keep_s"]
with open(args.out / "scores.tsv", "w") as f:
    f.write("\t".join(columns) + "\n")
    for r in rows:
        f.write("\t".join(fmt(r.get(c, "")) for c in columns) + "\n")


def mean(values: list[float]) -> float:
    values = [v for v in values if isinstance(v, float) and not math.isnan(v)]
    return sum(values) / len(values) if values else float("nan")


lines = ["### Kept regions against the clean photo (means over the matrix cases' boxes)", "",
         "| arm | MP | DISTS | LPIPS | HF vs photo | ΔE | zoom | shift px | HF whole | exec s | generating s |", "|---|---|---|---|---|---|---|---|---|---|---|"]
matrix = {c["name"] for c in plan["cases"].get("M", [])}
for arm in ["input", *arms]:
    for mp in sorted({str(r["mp"]) for r in rows}):
        sel = [r for r in rows if r["arm"] == arm and str(r["mp"]) == mp and r["case"] in matrix and "dists" in r]
        if not sel:
            continue
        runs_ = [r for r in rows if r["arm"] == arm and str(r["mp"]) == mp and r["case"] in matrix and r.get("box", 0) in (0, "")]
        lines.append(f"| {arm} | {mp} | {mean([r['dists'] for r in sel]):.4f} | {mean([r['lpips'] for r in sel]):.4f} | "
                     f"{mean([r['hf'] for r in sel]):.3f} | {mean([r['de'] for r in sel]):.2f} | {mean([r.get('scale', float('nan')) for r in sel]):.4f} | "
                     f"{mean([r.get('shift', float('nan')) for r in sel]):.1f} | {mean([r.get('hf_all', float('nan')) for r in runs_]):.0f} | "
                     f"{mean([r.get('exec_s', float('nan')) for r in runs_]):.1f} | "
                     f"{mean([r.get('gen_s', float('nan')) for r in runs_]):.1f} |")

# Chains.
chain_rows: list[dict[str, object]] = []
for chain in plan.get("chains", []):
    source = sources[chain["start"]]
    still = boxes(source["still"])
    zero = rgb(HERE / "chain" / f"{chain['start']}-0.png")
    for arm in [a["name"] for a in plan.get("chain_arms", [])]:
        folder = args.work / "chains" / chain["name"] / arm
        tiles = [("g0", crop(zero, still[0]))]
        for k in range(1, len(chain["steps"]) + 1):
            found = sorted(folder.glob(f"g{k}.*")) if folder.exists() else []
            if not found:
                break
            out = rgb(found[0])
            truth = zero.resize(out.size, Image.Resampling.LANCZOS)
            moved, scale, shift = align(out, truth, still)
            chain_rows.append({"chain": chain["name"], "arm": arm, "gen": k, "scale": scale, "shift": shift}
                              | held(moved, truth, still[0], False) | timing(f"chain-{chain['name']}--{arm}--g{k}"))
            tiles.append((f"g{k}", crop(out, still[0])))
        if len(tiles) > 1:
            sheet(tiles, args.out / "sheets" / f"chain-{chain['name']}--{arm}.jpg")
            sheet([tiles[0], tiles[-1]], args.out / "sheets" / f"chain-{chain['name']}--{arm}--200.png", zoom=2)
ccols = ["chain", "arm", "gen", "dists", "lpips", "hf", "de", "scale", "shift", "exec_s", "gen_s"]
with open(args.out / "chains.tsv", "w") as f:
    f.write("\t".join(ccols) + "\n")
    for r in chain_rows:
        f.write("\t".join(fmt(r.get(c, "")) for c in ccols) + "\n")
if chain_rows:
    lines += ["", "### Chains: the never-edited box against generation 0 (DISTS / HF / ΔE / zoom per generation)", "",
              "| chain | arm | " + " | ".join(f"g{k}" for k in range(1, 6)) + " |", "|---|---|" + "---|" * 5]
    for chain in plan.get("chains", []):
        for arm in [a["name"] for a in plan.get("chain_arms", [])]:
            sel = {r["gen"]: r for r in chain_rows if r["chain"] == chain["name"] and r["arm"] == arm}
            cells = [f"{sel[k]['dists']:.3f} / {sel[k]['hf']:.2f} / {sel[k]['de']:.1f} / {sel[k]['scale']:.3f}" if k in sel else "" for k in range(1, 6)]
            lines.append(f"| {chain['name']} | {arm} | " + " | ".join(cells) + " |")

# Sheets: each case's first box at 100%, the photo, the input and every arm.
for name, case in cases.items():
    for mp in ("2", "4"):
        tiles: list[tuple[str, Image.Image]] = []
        for arm in arms:
            found = sorted((runs / arm).glob(f"{name}--{mp}.*")) if (runs / arm).exists() else []
            if found:
                out = rgb(found[0])
                region = boxes(case.get("boxes") or sources[case["input"]]["keep"])[0] if case["kind"] != "none" else (0.25, 0.1, 0.75, 0.5)
                same = abs(out.width / out.height / 0.8 - 1) < 0.02
                if not tiles and same:
                    tiles.append(("photo", crop(rgb(HERE / "clean" / f"{case['input']}.png").resize(out.size, Image.Resampling.LANCZOS), region)))
                    tiles.append(("input", crop(rgb(HERE / "inputs" / f"{case['input']}.jpg").resize(out.size, Image.Resampling.LANCZOS), region)))
                tiles.append((arm, crop(out, region)))
        if tiles:
            sheet(tiles, args.out / "sheets" / f"{name}--{mp}.jpg")
(args.out / "summary.md").write_text("\n".join(lines) + "\n")
print("\n".join(lines))
