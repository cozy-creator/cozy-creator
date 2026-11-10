"""The project cell's frame check (e2e/cli/project.sh, cl-716 A2): which source frame each frame of a cut is.
Usage: frames.py <ffmpeg> <cut.mp4> <fps> <source.mp4>:<start>:<end> ...   (frames at <fps>, end exclusive, in edit order)
Every frame of the cut is compared (96x54 grey, mean squared error) with its source's frames near the intended one: a source
at <fps> intends frame n; one at another rate r intends its frame nearest in time to n/<fps> s, round(n*r/<fps>), as ffmpeg's fps filter picks. The nearest must be
the intended frame. Each frame is also hashed (md5 of the full decoded yuv420p picture): a copied frame equals its source's.
Prints each piece's first and last frame, every miss, and the total."""
import hashlib
import json
import subprocess
import sys
from fractions import Fraction
from typing import TypedDict

import numpy as np
import numpy.typing as npt

FF, CUT, FPS = sys.argv[1], sys.argv[2], Fraction(sys.argv[3])
W, H = 96, 54


class Stream(TypedDict):
    r_frame_rate: str
    width: int
    height: int


class Probed(TypedDict):
    streams: list[Stream]


# A video's rate, its frames' md5s, and its frames as small grey pictures (one row each).
Frames = tuple[Fraction, list[str], npt.NDArray[np.float64]]


def probe(video: str) -> tuple[Fraction, int, int]:
    out = subprocess.run([FF.replace("ffmpeg", "ffprobe"), "-v", "error", "-select_streams", "v:0", "-show_entries",
                          "stream=r_frame_rate,width,height", "-of", "json", video], capture_output=True, check=True).stdout
    probed: Probed = json.loads(out)
    s = probed["streams"][0]
    return Fraction(s["r_frame_rate"]), s["width"], s["height"]


def frames(video: str) -> Frames:
    rate, w, h = probe(video)
    raw = subprocess.run([FF, "-v", "error", "-i", video, "-map", "0:v:0", "-fps_mode", "passthrough", "-pix_fmt", "yuv420p",
                          "-f", "rawvideo", "-"], capture_output=True, check=True).stdout
    size = w * h * 3 // 2
    pics = [raw[i:i + size] for i in range(0, len(raw), size)]
    hashes = [hashlib.md5(p).hexdigest() for p in pics]
    small = subprocess.run([FF, "-v", "error", "-i", video, "-map", "0:v:0", "-fps_mode", "passthrough", "-vf",
                            f"scale={W}:{H},format=gray", "-f", "rawvideo", "-"], capture_output=True, check=True).stdout
    grey = np.frombuffer(small, np.uint8).reshape(-1, W * H).astype(np.float64)
    return rate, hashes, grey


cut_rate, cut_hashes, cut_grey = frames(CUT)
sources: dict[str, Frames] = {}
at, ok, exact, total = 0, True, 0, 0
for spec in sys.argv[4:]:
    source, first, last = spec.rsplit(":", 2)
    start, end = int(first), int(last)
    if source not in sources:
        sources[source] = frames(source)
    rate, hashes, grey = sources[source]
    for n in range(start, end):
        want = n if rate == FPS else int(n * rate / FPS + Fraction(1, 2))
        mine = cut_grey[at]
        near = [k for k in range(want - 3, want + 4) if 0 <= k < len(grey)]
        errors = {k: float(np.mean((mine - grey[k]) ** 2)) for k in near}
        best = min(errors, key=errors.__getitem__)
        same = cut_hashes[at] == hashes[want]
        exact += same
        total += 1
        good = best == want
        ok &= good
        if not good or n in (start, end - 1):
            shown = ", ".join(f"{k}:{e:.1f}" for k, e in sorted(errors.items()))
            print(f"cut frame {at} = {source.rsplit('/', 1)[-1]} frame {n} at {FPS} fps -> source frame {best} "
                  f"(want {want}) {'PASS' if good else 'FAIL'}; md5 {'equal' if same else 'differs'}; mse {shown}")
        at += 1
print(f"cut has {len(cut_hashes)} frames at {cut_rate} fps; pieces want {at}; {exact} of {total} md5-equal to their source frame")
ok &= len(cut_hashes) == at
print("frames", "PASS" if ok else "FAIL")
