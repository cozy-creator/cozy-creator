"""upstream-parity.sh's judge, on the runner: each of our outputs against its upstream reference.

    compare.py <cells.tsv> <upstream dir> <out dir>      a row per line of cells.tsv: name, kind, ours, reference[, extra]
    compare.py --self-check <picture>

Kinds: `image` (PSNR over RGB, DISTS, and a 2x crop sheet: ours | upstream | |difference| x4), `video` (our MP4's
frames against the reference's uint8 frames, and the ceiling: the reference through our encoder, x264 crf 14
veryfast yuv420p), `text` (identical, or the first character where they part), `green` (a cutout of the green picture
against its truth on white: PSNR overall and on the soft edge, and the green left in that edge), `rewrite` (the last
event's end against the duration asked).
"""

from __future__ import annotations

import json
import math
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image

_DISTS = None


def rgb(path: Path) -> np.ndarray:
    return np.asarray(Image.open(path).convert("RGB"), dtype=np.uint8)


def psnr(a: np.ndarray, b: np.ndarray, where: np.ndarray | None = None) -> float:
    d = (a.astype(np.float64) - b.astype(np.float64)) ** 2
    if where is not None:
        d = d[where]
    mse = float(d.mean()) if d.size else 0.0
    return math.inf if mse == 0 else 10 * math.log10(255**2 / mse)


def dists(a: np.ndarray, b: np.ndarray) -> float:
    global _DISTS
    import piq
    import torch

    if _DISTS is None:
        _DISTS = piq.DISTS()
    x, y = (torch.from_numpy(v).permute(2, 0, 1)[None].float() / 255 for v in (a, b))
    with torch.no_grad():
        return float(_DISTS(x, y))


def sheet(a: np.ndarray, b: np.ndarray, out: Path) -> None:
    """The centre 384 px of each at 2x, side by side, and their difference x4."""
    h, w = a.shape[:2]
    s = min(384, h, w)
    y, x = (h - s) // 2, (w - s) // 2
    crops = [a[y : y + s, x : x + s], b[y : y + s, x : x + s]]
    crops.append(np.clip(np.abs(crops[0].astype(int) - crops[1].astype(int)) * 4, 0, 255).astype(np.uint8))
    row = np.concatenate([np.repeat(np.repeat(c, 2, 0), 2, 1) for c in crops], axis=1)
    Image.fromarray(row).save(out)


def frames(mp4: Path) -> np.ndarray:
    probe = json.loads(subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries",
                                       "stream=width,height", "-of", "json", str(mp4)], capture_output=True,
                                      check=True, text=True).stdout)["streams"][0]
    w, h = probe["width"], probe["height"]
    raw = subprocess.run(["ffmpeg", "-v", "error", "-i", str(mp4), "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
                         capture_output=True, check=True).stdout
    return np.frombuffer(raw, np.uint8).reshape(-1, h, w, 3)


def encoded(reference: np.ndarray, at: Path) -> np.ndarray:
    """The reference's frames through our encoder (the Runtime's `_X264`: crf 14, veryfast, yuv420p), decoded."""
    t, h, w, _ = reference.shape
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{w}x{h}", "-r", "24",
                    "-i", "-", "-c:v", "libx264", "-crf", "14", "-preset", "veryfast", "-pix_fmt", "yuv420p", str(at)],
                   input=reference.tobytes(), check=True)
    return frames(at)


def video_psnr(a: np.ndarray, b: np.ndarray) -> float:
    n = min(len(a), len(b))
    return float(np.mean([psnr(a[i], b[i]) for i in range(n)]))


def compare(rows: list[list[str]], up: Path, out: Path) -> list[str]:
    lines = []
    for row in rows:
        name, kind, ours, ref = row[:4]
        try:
            if kind == "image":
                a, b = rgb(Path(ours)), rgb(up / ref)
                if a.shape != b.shape:
                    lines.append(f"{name}\timage\tSIZE ours {a.shape[1]}x{a.shape[0]} upstream {b.shape[1]}x{b.shape[0]}")
                    continue
                sheet(a, b, out / f"{name}.png")
                lines.append(f"{name}\timage\tPSNR {psnr(a, b):.2f} dB\tDISTS {dists(a, b):.4f}\t{a.shape[1]}x{a.shape[0]}")
            elif kind == "video":
                a, b = frames(Path(ours)), np.load(up / ref)
                ceiling = video_psnr(encoded(b, out / f"{name}-ceiling.mp4"), b)
                mid = len(b) // 2
                sheet(a[mid], b[mid], out / f"{name}-mid.png")
                extra = ""
                if len(row) > 4 and (up / row[4]).exists():
                    extra = f"\tvs F32-VAE reference {video_psnr(a, np.load(up / row[4])):.2f} dB"
                lines.append(f"{name}\tvideo\tPSNR {video_psnr(a, b):.2f} dB over {min(len(a), len(b))} frames "
                             f"(ours {a.shape}, upstream {b.shape})\tceiling {ceiling:.2f} dB{extra}\t"
                             f"DISTS mid {dists(a[mid], b[mid]):.4f}")
            elif kind == "text":
                a = json.loads(Path(ours).read_text())["texts"][0].strip()
                b = json.loads((up / "qwen3.json").read_text())[ref]["text"]
                if a == b:
                    lines.append(f"{name}\ttext\tIDENTICAL ({len(a)} chars)")
                else:
                    at = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
                    lines.append(f"{name}\ttext\tparts at char {at} of {len(a)}/{len(b)}: ours {a[at:at + 60]!r} "
                                 f"upstream {b[at:at + 60]!r}")
            elif kind == "green":
                a, truth = rgb(Path(ours)), rgb(up / ref)
                alpha = np.asarray(Image.open(up / "birefnet-green-truth-mask.png").convert("L"))
                edge = (alpha > 3) & (alpha < 252)
                spill = lambda x: float((x[..., 1].astype(float) - (x[..., 0].astype(float) + x[..., 2]) / 2)[edge].mean())  # noqa: E731
                sheet(a, truth, out / f"{name}.png")
                lines.append(f"{name}\tgreen\tPSNR {psnr(a, truth):.2f} dB, edge {psnr(a, truth, edge):.2f} dB "
                             f"({int(edge.sum())} px)\tgreen in edge {spill(a):+.2f} (truth {spill(truth):+.2f})\t"
                             f"DISTS {dists(a, truth):.4f}")
            elif kind == "rewrite":
                text = json.loads(Path(ours).read_text())["texts"][0].strip()
                if text.startswith("```"):
                    text = text.partition("\n")[2].rsplit("```", 1)[0]
                value = json.loads(text)
                if isinstance(value, dict) and isinstance(value.get("refined_prompt"), str):
                    value = json.loads(value["refined_prompt"])
                ends = [float(e["time_range_sec"].split("to")[-1]) for e in value["events"]]
                lines.append(f"{name}\trewrite\tasked {ref} s, events end at {ends[-1]:g} s "
                             f"({'MATCH' if math.isclose(ends[-1], float(ref)) else 'OFF'}), {len(ends)} events")
            else:
                lines.append(f"{name}\t{kind}\tunknown kind")
        except Exception as exc:  # a missing or unreadable output is a row, not the end of the judging
            lines.append(f"{name}\t{kind}\tERROR {type(exc).__name__}: {str(exc)[:300]}")
    return lines


if __name__ == "__main__":
    if sys.argv[1] == "--self-check":
        a = rgb(Path(sys.argv[2]))
        assert psnr(a, a) == math.inf and dists(a, a) < 1e-3, "PSNR or DISTS of a picture against itself"
        print(f"self-check: PSNR inf, DISTS {dists(a, a):.5f}")
        sys.exit(0)
    table, up, out = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
    out.mkdir(parents=True, exist_ok=True)
    rows = [line.split("\t") for line in table.read_text().splitlines() if line.strip()]
    for line in compare(rows, up, out):
        print(line)
