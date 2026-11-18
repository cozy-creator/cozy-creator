"""analyze.py FRAMES_PY OUT PAIRS_JSON: each pair of one request's renders, one window (`one`) and windowed (`r`), against
each other and against the plate they came from. PAIRS_JSON: [{"pair", "plate", "one", "r", "spans": [[a, b], ...]}] with the
windowed arm's latent spans. Writes OUT/<pair>.json and, per pair, frames/<pair>/ (plate | one | r at 960x540), crops/<pair>/
(one | r | the plate's region, at the output's own pixels) and diff/<pair>/ (|one - r| luma x4) at the start, middle and end
and at each overlap's first, middle and last frame, plus temporal-<pair>.png (each frame's change from the last).

Latent frame l covers video frames from 2.5 l (l < 2, the VAE's first 5 frames) or 5 + 3.4 (l - 2) on (17 frames a 5-latent
chunk). PSNR and SSIM between the arms are ffmpeg's, luma, at the output's size, per frame."""

import json
import os
import re
import subprocess
import sys

import numpy as np

frames_py, out, pairs_path = sys.argv[1:4]
sys.path.insert(0, os.path.dirname(frames_py))
import frames as fr  # noqa: E402  (release-gate/frames.py)

FPS = 24


def video_frame(latent: float) -> float:
    return 2.5 * latent if latent < 2 else 5 + (latent - 2) * 3.4


def probe(path: str) -> tuple[int, int, int]:
    p = fr.probe(path)
    return p["width"], p["height"], p["frames"]


def pick(path: str, at: list[int], width: int, height: int, crop: str = "") -> dict[int, np.ndarray]:
    """RGB frames `at` (indices), each scaled to width x height (after `crop`, in the video's own pixels)."""
    sel = "+".join(f"eq(n\\,{n})" for n in sorted(set(at)))
    chain = f"select='{sel}',{crop + ',' if crop else ''}scale={width}:{height}:flags=lanczos,format=rgb24"
    # Each selected frame once: `-fps_mode` since FFmpeg 5.1, `-vsync` before it.
    for sync in (["-fps_mode", "passthrough"], ["-vsync", "0"]):
        done = subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-i", path, "-vf", chain, *sync, "-f", "rawvideo", "-"], capture_output=True)
        if done.returncode == 0:
            break
    else:
        raise RuntimeError(f"ffmpeg select on {path}: {done.stderr.decode()[-400:]}")
    got = np.frombuffer(done.stdout, np.uint8).reshape(-1, height, width, 3)
    return dict(zip(sorted(set(at)), got, strict=True))


def png(path: str, rgb: np.ndarray) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    h, w = rgb.shape[:2]
    subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{w}x{h}", "-i", "-",
                    path], input=np.ascontiguousarray(rgb).tobytes(), check=True)


def per_frame(a: str, b: str, kind: str) -> list[float]:
    """ffmpeg's per-frame luma PSNR (dB) or SSIM of b against a."""
    log = os.path.join(out, f".{kind}.log")
    subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-i", a, "-i", b, "-lavfi", f"[0:v][1:v]{kind}=stats_file={log}", "-f",
                    "null", "-"], check=True)
    key = "psnr_y" if kind == "psnr" else "Y"
    vals = []
    for line in open(log):
        m = re.search(rf"\b{key}:([0-9.]+|inf)", line)
        if m:
            vals.append(99.0 if m.group(1) == "inf" else float(m.group(1)))
    os.remove(log)
    return vals


def change(path: str) -> np.ndarray:
    """Each frame's mean absolute luma change from the last, at 224x128 (frames.py flicker's measure)."""
    g = fr.gray(path, 224, 128)
    return np.abs(np.diff(g, axis=0)).mean(axis=(1, 2))


def detail_box(plate_frame: np.ndarray, bw: int, bh: int) -> tuple[int, int]:
    """The (x, y) of the bw x bh box of the plate frame with the most fine detail (mean |Laplacian| of luma)."""
    y = plate_frame.astype(np.float64) @ np.array([0.299, 0.587, 0.114])
    lap = np.abs(4 * y[1:-1, 1:-1] - y[:-2, 1:-1] - y[2:, 1:-1] - y[1:-1, :-2] - y[1:-1, 2:])
    best, at = -1.0, (0, 0)
    for yy in range(0, lap.shape[0] - bh, max(bh // 4, 1)):
        for xx in range(0, lap.shape[1] - bw, max(bw // 4, 1)):
            s = float(lap[yy:yy + bh, xx:xx + bw].mean())
            if s > best:
                best, at = s, (xx, yy)
    return at


def chart(path: str, series: dict[str, np.ndarray], shade: list[tuple[float, float]], title: str) -> None:
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    fig, ax = plt.subplots(figsize=(14, 4), dpi=110)
    for a, b in shade:
        ax.axvspan(a, b, color="#f2c14e", alpha=0.35, lw=0)
    for (name, d), color in zip(series.items(), ("#555555", "#1f77b4", "#d62728"), strict=False):
        ax.plot((np.arange(len(d)) + 1) / FPS, d, lw=1.1, color=color, label=name)
    ax.set_xlabel("seconds (shaded: the windowed arm's overlaps)")
    ax.set_ylabel("mean |luma change| from the last frame")
    ax.set_title(title)
    ax.legend(loc="upper right")
    fig.tight_layout()
    fig.savefig(path)
    plt.close(fig)


summary = []
for p in json.load(open(pairs_path)):
    name, plate, one, r = p["pair"], p["plate"], p["one"], p["r"]
    w, h, n = probe(one)
    n = min(n, probe(r)[2], probe(plate)[2])
    overlaps = [(b[0], a[1]) for a, b in zip(p["spans"], p["spans"][1:])]
    seams = [(round(video_frame(a)), min(round(video_frame(b)) - 1, n - 1)) for a, b in overlaps]
    inside = np.zeros(n, bool)
    for a, b in seams:
        inside[a:b + 1] = True
    psnr = np.array(per_frame(one, r, "psnr")[:n])
    ssim = np.array(per_frame(one, r, "ssim")[:n])
    worst = int(np.argmin(psnr))
    between = {"psnr_mean": round(float(psnr.mean()), 2), "psnr_min": round(float(psnr.min()), 2),
               "psnr_min_at_s": round(worst / FPS, 2), "ssim_mean": round(float(ssim.mean()), 4),
               "psnr_in_overlaps": round(float(psnr[inside].mean()), 2) if inside.any() else None,
               "psnr_outside": round(float(psnr[~inside].mean()), 2) if (~inside).any() else None}
    # Each windowed span's frames alone (its own window and its neighbours' shares).
    between["psnr_by_span"] = [round(float(psnr[round(video_frame(a)):min(round(video_frame(b)), n)].mean()), 2) for a, b in p["spans"]]
    plate_d, one_d, r_d = change(plate), change(one), change(r)
    m = min(len(plate_d), len(one_d), len(r_d))
    plate_d, one_d, r_d = plate_d[:m], one_d[:m], r_d[:m]
    ins = inside[1:m + 1]  # change k is frame k+1's

    def excess(d: np.ndarray) -> dict[str, float | None]:
        x = d / np.maximum(plate_d, 0.5)
        return {"change_vs_plate_in_overlaps": round(float(x[ins].mean()), 3) if ins.any() else None,
                "change_vs_plate_outside": round(float(x[~ins].mean()), 3) if (~ins).any() else None,
                "change_mean": round(float(d.mean()), 3)}

    gap = r_d - one_d
    g = int(np.argmax(gap))
    temporal = {"one": excess(one_d), "r": excess(r_d), "plate_change_mean": round(float(plate_d.mean()), 3),
                "r_minus_one_change_max": round(float(gap[g]), 3), "r_minus_one_change_max_at_s": round((g + 1) / FPS, 2),
                "r_minus_one_change_max_in_overlap": bool(ins[g])}
    mids = [round(((a + b) / 2) / FPS, 3) for a, b in seams]
    result = {"pair": name, "size": f"{w}x{h}", "frames": n, "spans": p["spans"], "overlaps_latent": overlaps,
              "seams_frames": seams, "seams_s": [[round(a / FPS, 2), round(b / FPS, 2)] for a, b in seams],
              "between": between, "temporal": temporal,
              "vs_plate": {"one": fr.regen(plate, one), "r": fr.regen(plate, r)}}
    if mids:
        result["seams_one"] = fr.seams(one, mids)
        result["seams_r"] = fr.seams(r, mids)
    json.dump(result, open(os.path.join(out, f"{name}.json"), "w"), indent=1)
    chart(os.path.join(out, f"temporal-{name}.png"), {"plate": plate_d, "one window": one_d, f"{len(p['spans'])} windows": r_d},
          [(a / FPS, b / FPS) for a, b in seams], f"{name}: frame-to-frame change")
    # Frames: start, middle, end, and each overlap's first, middle and last.
    tags = {"start": 0, "middle": n // 2, "end": n - 1}
    for i, (a, b) in enumerate(seams, 1):
        tags |= {f"seam{i}-first": a, f"seam{i}-middle": (a + b) // 2, f"seam{i}-last": b}
    at = list(tags.values())
    small = {k: pick(v, at, 960, 540) for k, v in (("plate", plate), ("one", one), ("r", r))}
    pw, ph, _ = probe(plate)
    # The crop: 800x450 of the output's own pixels where the plate has most detail at the middle frame.
    cw, ch = min(800, w), min(450, h)
    big = pick(plate, [n // 2], pw, ph)[n // 2]
    bx, by = detail_box(big, round(cw * pw / w), round(ch * ph / h))
    ox, oy = round(bx * w / pw), round(by * h / ph)
    crop = f"crop={cw}:{ch}:{ox}:{oy}"
    pcrop = f"crop={round(cw * pw / w)}:{round(ch * ph / h)}:{bx}:{by}"
    full = {"one": pick(one, at, cw, ch, crop), "r": pick(r, at, cw, ch, crop), "plate": pick(plate, at, cw, ch, pcrop)}
    gray = np.array([0.299, 0.587, 0.114])
    for tag, k in tags.items():
        png(os.path.join(out, "frames", name, f"{tag}-f{k}.png"), np.concatenate([small[x][k] for x in ("plate", "one", "r")], axis=1))
        png(os.path.join(out, "crops", name, f"{tag}-f{k}.png"), np.concatenate([full[x][k] for x in ("one", "r", "plate")], axis=1))
        d = np.abs(small["one"][k].astype(np.float64) @ gray - small["r"][k].astype(np.float64) @ gray) * 4
        png(os.path.join(out, "diff", name, f"{tag}-f{k}.png"), np.repeat(np.clip(d, 0, 255).astype(np.uint8)[..., None], 3, axis=2))
    result["crop"] = {"output_xy": [ox, oy], "size": [cw, ch]}
    result["frames_at"] = tags
    json.dump(result, open(os.path.join(out, f"{name}.json"), "w"), indent=1)
    summary.append(result)
    b, t = result["between"], result["temporal"]
    vo, vr = result["vs_plate"]["one"], result["vs_plate"]["r"]
    print(f"{name} {w}x{h} {n} frames, {len(p['spans'])} windows, overlaps at {result['seams_s']} s: between the arms PSNR "
          f"{b['psnr_mean']} dB (min {b['psnr_min']} at {b['psnr_min_at_s']} s; in overlaps {b['psnr_in_overlaps']}, outside "
          f"{b['psnr_outside']}), SSIM {b['ssim_mean']}; vs plate one {vo['psnr']}/{vo['ssim']} structure {vo['structure_psnr']}/"
          f"{vo['structure_ssim']} flicker {vo['flicker']} {vo['flicker_at_s']}, r {vr['psnr']}/{vr['ssim']} structure "
          f"{vr['structure_psnr']}/{vr['structure_ssim']} flicker {vr['flicker']} {vr['flicker_at_s']}; change vs plate in "
          f"overlaps one {t['one']['change_vs_plate_in_overlaps']} r {t['r']['change_vs_plate_in_overlaps']}, outside one "
          f"{t['one']['change_vs_plate_outside']} r {t['r']['change_vs_plate_outside']}; r - one change max "
          f"{t['r_minus_one_change_max']} at {t['r_minus_one_change_max_at_s']} s")
json.dump(summary, open(os.path.join(out, "pairs.json"), "w"), indent=1)
