"""diff.py <a> <b>: how two pictures differ, a channel at a time, as ffmpeg decodes them to rgb24 (walkthrough.sh's LoRA pairs): the
mean |Δ| of each channel and of all, the largest, the share of pixels where any channel moved by more than 8 of 255. JSON on stdout."""

import hashlib
import json
import subprocess
import sys


def decoded(path: str) -> tuple[int, int, bytes]:
    size = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0", path],
                          capture_output=True, text=True, check=True).stdout.strip().split(",")
    raw = subprocess.run(["ffmpeg", "-v", "error", "-i", path, "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
                         capture_output=True, check=True).stdout
    return int(size[0]), int(size[1]), raw


def main() -> int:
    a_path, b_path = sys.argv[1], sys.argv[2]
    aw, ah, a = decoded(a_path)
    bw, bh, b = decoded(b_path)
    said: dict[str, object] = {
        "a": {"file": a_path.rsplit("/", 1)[-1], "size": [aw, ah], "sha256": hashlib.sha256(open(a_path, "rb").read()).hexdigest()[:12]},
        "b": {"file": b_path.rsplit("/", 1)[-1], "size": [bw, bh], "sha256": hashlib.sha256(open(b_path, "rb").read()).hexdigest()[:12]},
    }
    if (aw, ah) != (bw, bh) or len(a) != len(b) or not a:
        said["comparable"] = False
        print(json.dumps(said))
        return 1
    sums, most, moved = [0, 0, 0], 0, 0
    for i in range(0, len(a), 3):
        r, g, bl = abs(a[i] - b[i]), abs(a[i + 1] - b[i + 1]), abs(a[i + 2] - b[i + 2])
        sums[0] += r
        sums[1] += g
        sums[2] += bl
        top = max(r, g, bl)
        most = max(most, top)
        moved += top > 8
    pixels = len(a) // 3
    said.update({
        "comparable": True, "same_bytes": a == b,
        "mean": round(sum(sums) / len(a), 3), "mean_rgb": [round(s / pixels, 3) for s in sums], "max": most,
        "moved_share": round(moved / pixels, 4),
    })
    print(json.dumps(said))
    return 0


if __name__ == "__main__":
    sys.exit(main())
