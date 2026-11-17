"""orbit-arcs.sh's measurer (se-152). Modes:

video <mp4> <dir> [--key white]  stills at 0, 1/8 ... 1 of the clip, a sheet of them, and its numbers (numbers.json and the
                                 last line): frames, size, seam (first vs last frame) and step PSNR on 96x54 grey (loopcut.py's),
                                 and the backdrop's luma (the outer 4% ring) over the clip. --key keys the clip's own plain
                                 backdrop (its middle frames' ring colour) to white in the stills and a second sheet.
grey <picture> <out>             a picture's white backdrop, flooded from its edges, made ORB360's mid grey.
hflip <picture> <out>            a picture mirrored.
"""

from __future__ import annotations

import json
import math
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

GREY = (128, 128, 128)
SHOTS = 9


def probe(path: Path) -> dict[str, object]:
    out = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-count_frames", "-show_entries",
                          "stream=width,height,nb_read_frames,r_frame_rate", "-of", "json", str(path)],
                         check=True, capture_output=True, text=True).stdout  # fmt: skip
    return json.loads(out)["streams"][0]


def frames(path: Path, width: int, height: int) -> np.ndarray:
    raw = subprocess.run(["ffmpeg", "-v", "error", "-i", str(path), "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
                         check=True, capture_output=True).stdout  # fmt: skip
    return np.frombuffer(raw, dtype=np.uint8).reshape(-1, height, width, 3)


def grey96(frame: np.ndarray) -> np.ndarray:
    return np.asarray(Image.fromarray(frame).convert("L").resize((96, 54)), dtype=np.float32)


def psnr(a: np.ndarray, b: np.ndarray) -> float:
    mse = float(np.mean((a - b) ** 2))
    return 99.0 if mse == 0 else 10 * math.log10(255**2 / mse)


def ring(frame: np.ndarray) -> np.ndarray:
    h, w = frame.shape[:2]
    e = max(2, round(min(h, w) * 0.04))
    return np.concatenate([frame[:e].reshape(-1, 3), frame[-e:].reshape(-1, 3), frame[:, :e].reshape(-1, 3),
                           frame[:, -e:].reshape(-1, 3)]).astype(np.float32)  # fmt: skip


def luma(pixels: np.ndarray) -> float:
    return float(np.mean(pixels @ np.array([0.299, 0.587, 0.114], dtype=np.float32)))


def keyed(frame: np.ndarray, backdrop: np.ndarray, to: tuple[int, int, int]) -> np.ndarray:
    """The frame with what is near the backdrop's colour (soft, by distance) made `to`."""
    distance = np.linalg.norm(frame.astype(np.float32) - backdrop, axis=2)
    alpha = np.clip((distance - 18) / 30, 0, 1)[..., None]  # 0 = backdrop, 1 = subject
    return (frame * alpha + np.array(to, dtype=np.float32) * (1 - alpha)).astype(np.uint8)


def sheet(stills: list[tuple[int, np.ndarray]], total: int, path: Path) -> None:
    tall = 360
    tiles = []
    font = ImageFont.load_default(size=22)
    for index, frame in stills:
        tile = Image.fromarray(frame)
        tile = tile.resize((round(tile.width * tall / tile.height), tall))
        draw = ImageDraw.Draw(tile)
        label = f"{index}/{total - 1}"
        draw.rectangle((0, 0, 12 + 12 * len(label), 30), fill=(0, 0, 0))
        draw.text((6, 3), label, fill=(255, 255, 255), font=font)
        tiles.append(tile)
    across = 5 if len(tiles) > 5 else len(tiles)
    rows = math.ceil(len(tiles) / across)
    wide = max(tile.width for tile in tiles)
    out = Image.new("RGB", (across * wide + (across - 1) * 6, rows * tall + (rows - 1) * 6), (40, 40, 40))
    for k, tile in enumerate(tiles):
        out.paste(tile, ((k % across) * (wide + 6), (k // across) * (tall + 6)))
    out.save(path, quality=88)


def video(path: Path, out: Path, key: str | None) -> None:
    out.mkdir(parents=True, exist_ok=True)
    info = probe(path)
    width, height = int(info["width"]), int(info["height"])  # type: ignore[call-overload]
    clip = frames(path, width, height)
    n = len(clip)
    picks = sorted({round(k * (n - 1) / (SHOTS - 1)) for k in range(SHOTS)})
    stills = [(i, clip[i]) for i in picks]
    for i, frame in stills:
        Image.fromarray(frame).save(out / f"{i:04d}.jpg", quality=92)
    sheet(stills, n, out / "sheet.jpg")
    small = [grey96(frame) for frame in clip]
    steps = [psnr(small[i], small[i + 1]) for i in range(n - 1)]
    rings = [luma(ring(frame)) for frame in clip]
    middle = rings[n // 8 : n - n // 8] or rings
    numbers: dict[str, object] = {
        "frames": n, "size": f"{width}x{height}", "fps": info["r_frame_rate"],
        "seam_db": round(psnr(small[0], small[-1]), 2),
        "step_db_median": round(float(np.median(steps)), 2), "step_db_min": round(min(steps), 2),
        "step_min_at": int(np.argmin(steps)),
        "backdrop_luma": {"first": round(rings[0], 1), "last": round(rings[-1], 1), "middle_min": round(min(middle), 1),
                          "middle_median": round(float(np.median(middle)), 1)},
    }  # fmt: skip
    if key:
        backdrop = np.median(np.concatenate([ring(clip[i]) for i in range(n // 4, n - n // 4, max(1, n // 16))]), axis=0)
        to = (255, 255, 255) if key == "white" else GREY
        made = [(i, keyed(frame, backdrop, to)) for i, frame in stills]
        for i, frame in made:
            Image.fromarray(frame).save(out / f"{i:04d}-keyed.jpg", quality=92)
        sheet(made, n, out / "sheet-keyed.jpg")
        numbers["keyed_backdrop_rgb"] = [round(float(c)) for c in backdrop]
    (out / "numbers.json").write_text(json.dumps(numbers, indent=1))
    print(json.dumps(numbers))


def grey(source: Path, target: Path) -> None:
    picture = Image.open(source).convert("RGB")
    w, h = picture.size
    seeds = [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1), (w // 2, 0), (w // 2, h - 1), (0, h // 2), (w - 1, h // 2)]
    for seed in seeds:
        if min(picture.getpixel(seed)) > 225:  # type: ignore[arg-type]
            ImageDraw.floodfill(picture, seed, GREY, thresh=40)
    picture.save(target)
    print(f"{target}: {w}x{h}, edge luma now {luma(ring(np.asarray(picture))):.1f}")


def hflip(source: Path, target: Path) -> None:
    Image.open(source).convert("RGB").transpose(Image.Transpose.FLIP_LEFT_RIGHT).save(target)


if __name__ == "__main__":
    mode, *rest = sys.argv[1:]
    if mode == "video":
        key = rest[rest.index("--key") + 1] if "--key" in rest else None
        video(Path(rest[0]), Path(rest[1]), key)
    elif mode == "grey":
        grey(Path(rest[0]), Path(rest[1]))
    elif mode == "hflip":
        hflip(Path(rest[0]), Path(rest[1]))
    else:
        sys.exit(f"unknown mode {mode}")
