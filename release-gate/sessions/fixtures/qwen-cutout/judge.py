"""A transparency request's output, judged (se-154): judge.py CELLS.tsv --sheets DIR --out TSV, or --self-check PICTURE.

CELLS.tsv rows: name, input picture (or empty), output picture, its run record (`cozy run watch --json --full`). For
each output, besides the alpha below: the rewrite the model read (the result's `prompt`), whether it took the model
card's template ("This is an RGBA image with transparency. ... The image has alpha channel and the background is
transparent.": both phrases, one, or none), the enhancer's seconds (stage `upsampling prompt`) and a painted
checkerboard (grey_noise >= CHECKER). It measures and never fails a session. For each output:
- clear, solid, partial: the share of pixels with alpha <= 8, >= 247, and between. A cutout has a cleared background, a
  solid subject, and partial alpha only along its edge (Paul's v8, run PRO6-29: none clear, half its picture at 222-241).
- edge, the edge-noise metric: the solid mask's (alpha > 127) boundary pixels over the square root of its area. About 3.5
  for a disk, under 15 for a figure with curly hair, hundreds for a mask frayed or speckled by noise.
- grey_noise: the share of 16 px tiles of the picture over mid-grey that are colourless and busy (a painted checkerboard).
A cell is clean with clear >= 5%, solid >= 5%, partial <= 10% and edge <= 40.
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path

import numpy as np
import numpy.typing as npt
from PIL import Image, ImageDraw

#: (metric, at least, at most)
BARS = (("clear", 0.05, 1.0), ("solid", 0.05, 1.0), ("partial", 0.0, 0.10), ("edge", 0.0, 40.0))
TILE = 16
#: grey_noise at and past this is a painted checkerboard: red's (44186728100, man seed 7) read 0.267, clean ones 0.02-0.07.
CHECKER = 0.15
#: The model card's two phrases (HF Qwen/Qwen-Image-2.1 README, "Transparent Image Generation (RGBA)").
CARD = ("this is an rgba image with transparency", "the image has alpha channel and the background is transparent")
Floats = npt.NDArray[np.float32]


@dataclass(frozen=True)
class Judged:
    mode: str
    values: dict[str, float]

    def line(self) -> str:
        return f"{self.mode} " + " ".join(f"{k} {v:.3f}" for k, v in self.values.items())

    def missed(self) -> list[str]:
        if self.mode not in ("RGBA", "LA"):
            return [f"{self.mode}: no alpha"]
        return [f"{k} {self.values[k]:.3f} not in [{lo}, {hi}]" for k, lo, hi in BARS if not lo <= self.values[k] <= hi]


def tiles(x: Floats) -> Floats:
    h, w = (x.shape[0] // TILE) * TILE, (x.shape[1] // TILE) * TILE
    return x[:h, :w].reshape(h // TILE, TILE, w // TILE, TILE).mean(axis=(1, 3))


def judge(im: Image.Image) -> Judged:
    rgba = np.asarray(im.convert("RGBA"), dtype=np.float32)
    a = rgba[..., 3]
    clear, solid = float((a <= 8).mean()), float((a >= 247).mean())
    mask = a > 127
    inner = mask.copy()
    inner[1:, :] &= mask[:-1, :]
    inner[:-1, :] &= mask[1:, :]
    inner[:, 1:] &= mask[:, :-1]
    inner[:, :-1] &= mask[:, 1:]
    edge = float((mask & ~inner).sum()) / max(1.0, float(np.sqrt(mask.sum())))
    over = rgba[..., :3] * (a[..., None] / 255) + 128 * (1 - a[..., None] / 255)
    lum = over.mean(axis=-1)
    lap = np.zeros_like(lum)
    lap[1:-1, 1:-1] = np.abs(4 * lum[1:-1, 1:-1] - lum[:-2, 1:-1] - lum[2:, 1:-1] - lum[1:-1, :-2] - lum[1:-1, 2:])
    sat = over.max(axis=-1) - over.min(axis=-1)
    grey = float(((tiles(sat.astype(np.float32)) < 24) & (tiles(lap.astype(np.float32)) > 12)).mean())
    return Judged(im.mode, {"clear": clear, "solid": solid, "partial": 1 - clear - solid, "edge": edge, "grey_noise": grey})


def checker(size: tuple[int, int], cell: int = 16) -> Image.Image:
    w, h = size
    y, x = np.mgrid[:h, :w]
    light = ((x // cell + y // cell) % 2) == 0
    return Image.fromarray(np.where(light[..., None], 204, 153).repeat(3, axis=-1).astype(np.uint8))


def sheet(name: str, src: str, out: Image.Image, line: str, to: Path, height: int = 384) -> None:
    """The input, the output over white, black and a checkerboard, and its alpha, side by side."""
    rgba = out.convert("RGBA")
    rgba = rgba.resize((round(rgba.width * height / rgba.height), height), Image.Resampling.LANCZOS)
    shown: list[Image.Image] = []
    if src:
        inp = Image.open(src).convert("RGB")
        shown.append(inp.resize((round(inp.width * height / inp.height), height), Image.Resampling.LANCZOS))
    for ground in (Image.new("RGB", rgba.size, "white"), Image.new("RGB", rgba.size, "black"), checker(rgba.size)):
        shown.append(Image.alpha_composite(ground.convert("RGBA"), rgba).convert("RGB"))
    shown.append(rgba.getchannel("A").convert("RGB"))
    canvas = Image.new("RGB", (sum(t.width for t in shown) + 6 * len(shown), height + 22), (40, 40, 40))
    x = 0
    for t in shown:
        canvas.paste(t, (x, 22))
        x += t.width + 6
    ImageDraw.Draw(canvas).text((4, 4), f"{name}: {line}", fill=(255, 255, 255))
    canvas.save(to / f"{name}.png")


def self_check(picture: str) -> int:
    """A clean elliptical cutout of PICTURE passes; the same with a noisy, half-transparent alpha fails."""
    rgb = np.asarray(Image.open(picture).convert("RGB"))
    h, w = rgb.shape[:2]
    y, x = np.mgrid[:h, :w]
    r = np.sqrt(((x - w / 2) / (w / 3)) ** 2 + ((y - h / 2) / (h / 2.5)) ** 2)
    soft = np.clip((1.02 - r) / 0.04, 0, 1)
    noise = np.random.default_rng(0).normal(0, 20, soft.shape)
    clean = judge(Image.fromarray(np.dstack([rgb, (soft * 255).astype(np.uint8)])))
    noisy = judge(Image.fromarray(np.dstack([rgb, np.clip(200 + 55 * soft + noise, 0, 255).astype(np.uint8)])))
    print(f"clean: {clean.line()} -> {'FAIL' if clean.missed() else 'PASS'}")
    print(f"noisy: {noisy.line()} -> {'FAIL' if noisy.missed() else 'PASS'}")
    return 0 if not clean.missed() and noisy.missed() else 1


def stage_ms(record: object, name: str) -> float | None:
    """The total of stage `name` anywhere in a run record (the run's own measurements, or a call's)."""
    if isinstance(record, dict):
        stages = record.get("stages")
        if isinstance(stages, dict) and isinstance(stages.get(name), dict):
            return float(stages[name].get("total_ms", 0.0))
        for value in record.values():
            found = stage_ms(value, name)
            if found is not None:
                return found
    elif isinstance(record, list):
        for value in record:
            found = stage_ms(value, name)
            if found is not None:
                return found
    return None


def template(rewrite: str) -> str:
    flat = " ".join(rewrite.lower().split())
    hits = sum(phrase in flat for phrase in CARD)
    return "both" if hits == 2 else "one" if hits else "none"


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("cells", nargs="?")
    p.add_argument("--sheets")
    p.add_argument("--out")
    p.add_argument("--self-check")
    args = p.parse_args()
    if args.self_check:
        return self_check(args.self_check)
    sheets = Path(args.sheets) if args.sheets else None
    if sheets:
        sheets.mkdir(parents=True, exist_ok=True)
    rows: list[str] = []
    counts = {"cells": 0, "rgba": 0, "clean": 0, "checker": 0, "template": 0}
    for row in Path(args.cells).read_text().splitlines():
        name, src, out, rec = (row.split("\t") + [""] * 4)[:4]
        if not out or not Path(out).is_file():
            print(f"NONE {name}: no output")
            continue
        im = Image.open(out)
        j = judge(im)
        record = json.loads(Path(rec).read_text()) if rec and Path(rec).is_file() else {}
        result = record.get("result") if isinstance(record, dict) else None
        rewrite = str(result.get("prompt", "")) if isinstance(result, dict) else ""
        enhancer = stage_ms(record, "upsampling prompt")
        took = template(rewrite)
        checker = j.values["grey_noise"] >= CHECKER
        mark = "CLEAN" if not j.missed() else "NOT"
        counts["cells"] += 1
        counts["rgba"] += j.mode == "RGBA"
        counts["clean"] += mark == "CLEAN"
        counts["checker"] += checker
        counts["template"] += took == "both"
        line = (f"{j.line()} template {took} enhancer "
                f"{'-' if enhancer is None else f'{enhancer / 1000:.1f}s'}")
        print(f"{mark:5} {name} {im.width}x{im.height} {line}")
        print(f"      rewrite: {rewrite[:1500]}")
        rows.append("\t".join([name, mark, j.mode, *(f"{v:.4f}" for v in j.values.values()), "yes" if checker else "no", took,
                               "" if enhancer is None else f"{enhancer / 1000:.1f}", rewrite.replace("\t", " ").replace("\n", " ")]))
        if sheets:
            sheet(name, src, im, f"{mark} {line}", sheets)
    if args.out:
        Path(args.out).write_text("name\tverdict\tmode\tclear\tsolid\tpartial\tedge\tgrey_noise\tcheckerboard\ttemplate\tenhancer_s\trewrite\n"
                                  + "\n".join(rows) + "\n")
    print("of {cells}: {rgba} RGBA, {clean} clean, {template} rewrites took the card's template (checkerboards: by eye)"
          .format(**counts))
    return 0


if __name__ == "__main__":
    sys.exit(main())
