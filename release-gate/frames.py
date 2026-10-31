"""One output video against the video it came from, as one JSON line (suites/h3).

    frames.py regen ORIGINAL OUTPUT                        # an upscale against its 768p original
    frames.py guide SOURCE OUTPUT [--guided S] [--fps24]   # a guided render against its guide clip
    frames.py still IMAGE OUTPUT first|last                # a keyframe against the output's first or last frame
    frames.py variant CONTROL OUTPUT                       # a fine-tune's or LoRA's render against a same-seed control

regen: both in the original's 1344x768 frame (a 16:9 output un-stretched), over every 4th frame: mean luma PSNR and
SSIM at full size (reported: regenerated texture, dense foliage most, rightly moves them), and at 112x64, where texture
averages out and only the layout is left (structure_psnr, structure_ssim: judged); plus the soundtrack's lag (ms) and
correlation at zero lag against the original's. `flicker`: the frames (at 224x128) whose change from the previous one
is over 3x the original's there and over 4 luma levels, with the first ones' times (`flicker_at_s`). An output that
moves as its original does scores 0; a refine window off H3's latent chunk grid flickers at every chunk (run 22567: 28).
guide: gray frames at 192x108 over the first S seconds (all of them by default); --fps24 first retimes the source to
the 24-fps clock by timestamp. Same-index SSIM, the share of every 3rd frame whose best-matching source frame within
+-12 is within +-1 (an output that follows its source frame by frame scores 1), and after S seconds the mean SSIM of
consecutive output frames (coherence: a render that falls apart scores low). Plus the output's size and frame count.
still: gray at 192x108, the image cropped to the output's aspect: SSIM of the keyframe against the output's frame at
that end (`ssim`) and at the other end (`other`, a control), and the least SSIM of consecutive frames over the last
(or first) half second (`seam`: a keyframe pasted on as a cut scores low).
variant: gray at 192x108 over every 4th frame: SSIM against the control (`ssim`, and over each half: `ssim_first`,
`ssim_second`, so a LoRA that reaches only one piece of a two-piece shot shows), the mean per-frame luma deviation of
the output and of the control (`contrast`, `control_contrast`: a grey render scores near 0), and over consecutive output
frames the mean SSIM (`coherence`) and the least one with its time (`seam`, `seam_at_s`: a cut between pieces scores low).
"""

import json
import subprocess
import sys

import numpy as np

FPS = 24


def probe(path: str) -> dict:
    out = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-count_frames", "-show_entries",
                          "stream=width,height,nb_read_frames", "-of", "json", path], capture_output=True, check=True)
    s = json.loads(out.stdout)["streams"][0]
    return {"width": int(s["width"]), "height": int(s["height"]), "frames": int(s["nb_read_frames"])}


def gray(path: str, width: int, height: int, vf: str = "") -> np.ndarray:
    chain = f"{vf}{',' if vf else ''}scale={width}:{height}:flags=area,format=gray"
    raw = subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-i", path, "-vf", chain, "-f", "rawvideo", "-"],
                         capture_output=True, check=True).stdout
    return np.frombuffer(raw, np.uint8).reshape(-1, height, width).astype(np.float64)


def ssim(a: np.ndarray, b: np.ndarray) -> float:
    """Mean SSIM with an 8x8 box window."""
    c1, c2, k = (0.01 * 255) ** 2, (0.03 * 255) ** 2, 8

    def box(z: np.ndarray) -> np.ndarray:
        s = np.pad(z.cumsum(-1).cumsum(-2), [(0, 0)] * (z.ndim - 2) + [(1, 0), (1, 0)])
        return (s[..., k:, k:] - s[..., :-k, k:] - s[..., k:, :-k] + s[..., :-k, :-k]) / (k * k)

    ma, mb = box(a), box(b)
    va, vb, cov = box(a * a) - ma**2, box(b * b) - mb**2, box(a * b) - ma * mb
    return float((((2 * ma * mb + c1) * (2 * cov + c2)) / ((ma**2 + mb**2 + c1) * (va + vb + c2))).mean())


def pcm(path: str) -> np.ndarray:
    raw = subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-i", path, "-vn", "-ac", "1", "-ar", "8000",
                          "-f", "f32le", "-"], capture_output=True, check=True).stdout
    return np.frombuffer(raw, np.float32)


def similarity(original: str, output: str, width: int, height: int) -> tuple[float, float]:
    """Mean luma PSNR and SSIM over every 4th frame, both scaled to width x height."""
    a, b = gray(original, width, height), gray(output, width, height)
    n = min(len(a), len(b))
    mse = [float(np.mean((a[i] - b[i]) ** 2)) for i in range(0, n, 4)]
    return (round(float(np.mean([99.0 if e == 0 else 10 * np.log10(255**2 / e) for e in mse])), 2),
            round(float(np.mean([ssim(a[i], b[i]) for i in range(0, n, 4)])), 3))


def flicker(original: str, output: str) -> tuple[int, list[float]]:
    """Frames whose change from the previous one is over 3x the original's and over 4 luma levels."""
    a, b = gray(original, 224, 128), gray(output, 224, 128)
    n = min(len(a), len(b))
    da = np.abs(np.diff(a[:n], axis=0)).mean(axis=(1, 2))
    db = np.abs(np.diff(b[:n], axis=0)).mean(axis=(1, 2))
    frames = np.where((db > 3 * np.maximum(da, 0.5)) & (db > 4))[0] + 1
    return len(frames), [round(int(f) / FPS, 2) for f in frames[:8]]


def regen(original: str, output: str) -> dict:
    psnr, ss = similarity(original, output, 1344, 768)
    structure_psnr, structure_ssim = similarity(original, output, 112, 64)
    flickers, flicker_at_s = flicker(original, output)
    x, y = pcm(original), pcm(output)
    m = min(len(x), len(y))
    lag, corr = None, None
    if m > 800:
        x, y = x[:m] - x[:m].mean(), y[:m] - y[:m].mean()
        size = 1 << (2 * m - 1).bit_length()
        c = np.fft.irfft(np.fft.rfft(y, size) * np.conj(np.fft.rfft(x, size)), size)
        c = np.concatenate([c[-(m - 1):], c[:m]])
        lag = (int(np.argmax(c)) - (m - 1)) / 8.0
        corr = round(float(np.dot(x, y) / (np.linalg.norm(x) * np.linalg.norm(y) + 1e-12)), 4)
    return {**probe(output), "psnr": psnr, "ssim": ss, "structure_psnr": structure_psnr, "structure_ssim": structure_ssim,
            "lag_ms": lag, "corr": corr, "flicker": flickers, "flicker_at_s": flicker_at_s}


def guide(source: str, output: str, guided: float | None, fps24: bool) -> dict:
    src, out = gray(source, 192, 108, f"fps={FPS}" if fps24 else ""), gray(output, 192, 108)
    n = min(len(out), len(src), len(out) if guided is None else round(guided * FPS))
    same = [ssim(out[i], src[i]) for i in range(n)]
    offsets = []
    for i in range(0, n, 3):
        window = range(max(0, i - 12), min(len(src), i + 13))
        offsets.append(max(window, key=lambda j: ssim(out[i], src[j])) - i)
    after = [ssim(out[i], out[i + 1]) for i in range(n, len(out) - 1)]
    return {**probe(output), "guided_frames": n, "ssim": round(float(np.mean(same)), 3),
            "within_1": round(float(np.mean(np.abs(offsets) <= 1)), 3),
            "coherence_after": round(float(np.mean(after)), 3) if after else None}


def still(image: str, output: str, at: str) -> dict:
    info = probe(output)
    w, h = info["width"], info["height"]
    crop = f"scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h}"
    key = gray(image, 192, 108, crop)[0]
    out = gray(output, 192, 108)
    end, other = (out[0], out[-1]) if at == "first" else (out[-1], out[0])
    edge = out[: FPS // 2 + 1] if at == "first" else out[-(FPS // 2 + 1):]
    return {**info, "ssim": round(ssim(key, end), 3), "other": round(ssim(key, other), 3),
            "seam": round(min(ssim(edge[i], edge[i + 1]) for i in range(len(edge) - 1)), 3)}


def variant(control: str, output: str) -> dict:
    a, b = gray(control, 192, 108), gray(output, 192, 108)
    n = min(len(a), len(b))
    pair = lambda r: round(float(np.mean([ssim(a[i], b[i]) for i in r])), 3)
    cons = [ssim(b[i], b[i + 1]) for i in range(len(b) - 1)]
    k = int(np.argmin(cons))
    return {**probe(output), "ssim": pair(range(0, n, 4)), "ssim_first": pair(range(0, n // 2, 4)),
            "ssim_second": pair(range(n // 2, n, 4)), "contrast": round(float(np.mean([b[i].std() for i in range(0, n, 4)])), 1),
            "control_contrast": round(float(np.mean([a[i].std() for i in range(0, n, 4)])), 1),
            "coherence": round(float(np.mean(cons)), 3), "seam": round(cons[k], 3), "seam_at_s": round((k + 1) / FPS, 2)}


if __name__ == "__main__":
    mode, first, second, *rest = sys.argv[1:]
    if mode == "regen":
        print(json.dumps(regen(first, second)))
    elif mode == "still":
        print(json.dumps(still(first, second, rest[0])))
    elif mode == "variant":
        print(json.dumps(variant(first, second)))
    else:
        guided = float(rest[rest.index("--guided") + 1]) if "--guided" in rest else None
        print(json.dumps(guide(first, second, guided, "--fps24" in rest)))
