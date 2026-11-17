"""Scores h3-parity's pairs on the runner (se-156): PSNR and DISTS on pixels, relative L2 on video latents, SNR on sound,
and 100% crops side by side.

    python compare.py selftest <dir>
    python compare.py pairs <session OUT> <packages checkout> '<[[a, b, what], ...]>'

A name is one of our cases (OUT/ours/<name>: an mp4 and its latents, or a png) or an upstream one
(OUT/upstream/<group>/<name>: frames.npy, latents.npy, audio.npy, rate.txt, or image.png). For a video pair whose b is
upstream, two more rows: b through our decode's cell equalizer (minimax-h3 `cell_texture`), and b through our encoder
(x264 crf 14), the floor an mp4 of ours can reach.
"""

import glob
import importlib.util
import json
import math
import os
import subprocess
import sys

import numpy as np
from PIL import Image

_DISTS = None


def dists(a, b):
    """DISTS of two (N, H, W, 3) uint8 stacks, each frame at most 1024 px on its long edge; None without piq."""
    global _DISTS
    try:
        import piq
        import torch
    except ImportError:
        return None
    if _DISTS is None:
        _DISTS = piq.DISTS(reduction="none")
    out = []
    for x, y in zip(a, b):
        scale = min(1.0, 1024 / max(x.shape[:2]))
        size = (round(x.shape[1] * scale), round(x.shape[0] * scale))
        t = [torch.from_numpy(np.asarray(Image.fromarray(v).resize(size, Image.Resampling.LANCZOS))).permute(2, 0, 1)[None].float() / 255
             for v in (x, y)]
        with torch.no_grad():
            out.append(float(_DISTS(t[0], t[1])))
    return float(np.mean(out))


def psnr(a, b):
    mse = np.mean((a.astype(np.float64) - b.astype(np.float64)) ** 2)
    return 99.0 if mse == 0 else 10 * math.log10(255.0**2 / mse)


def video_frames(path):
    probe = json.loads(subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height",
                                       "-of", "json", path], capture_output=True, check=True).stdout)["streams"][0]
    raw = subprocess.run(["ffmpeg", "-v", "error", "-i", path, "-f", "rawvideo", "-pix_fmt", "rgb24", "-"], capture_output=True,
                         check=True).stdout
    return np.frombuffer(raw, np.uint8).reshape(-1, probe["height"], probe["width"], 3)


def video_audio(path, rate):
    raw = subprocess.run(["ffmpeg", "-v", "error", "-i", path, "-vn", "-f", "f32le", "-ac", "2", "-ar", str(rate), "-"],
                         capture_output=True).stdout
    return np.frombuffer(raw, np.float32).reshape(-1, 2).T if raw else None


def x264(frames, where):
    """`frames` through our encoder's settings (Runtime `_codec`: x264 crf 14 veryfast yuv420p) and back."""
    t, h, w, _ = frames.shape
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{w}x{h}", "-r", "24", "-i", "-",
                    "-c:v", "libx264", "-crf", "14", "-preset", "veryfast", "-pix_fmt", "yuv420p", where],
                   input=frames.tobytes(), check=True)
    return video_frames(where)


def load(out, name):
    """A case's outputs: {kind, frames|image, latents?, audio?, rate?}."""
    ours = os.path.join(out, "ours", name)
    if os.path.isdir(ours):
        files = [f for f in glob.glob(os.path.join(ours, "**", "*"), recursive=True) if os.path.isfile(f)]
        mp4 = [f for f in files if f.endswith(".mp4")]
        if mp4:
            got = {"kind": "video", "frames": video_frames(mp4[0]), "mp4": mp4[0]}
            for f in files:
                if f.endswith(".mp4"):
                    continue
                try:
                    from safetensors.numpy import load_file

                    tensors = load_file(f)
                except Exception:
                    continue
                if "latents" in tensors:
                    got["latents"] = tensors["latents"].astype(np.float32)
            return got
        png = [f for f in files if f.lower().endswith((".png", ".webp", ".jpg"))]
        return {"kind": "image", "image": np.asarray(Image.open(png[0]).convert("RGB"))}
    hits = glob.glob(os.path.join(out, "upstream", "*", name))
    if not hits:
        raise FileNotFoundError(name)
    up = hits[0]
    if os.path.exists(os.path.join(up, "frames.npy")):
        return {"kind": "video", "frames": np.load(os.path.join(up, "frames.npy")), "latents": np.load(os.path.join(up, "latents.npy")),
                "audio": np.load(os.path.join(up, "audio.npy")), "rate": int(open(os.path.join(up, "rate.txt")).read()), "upstream": True}
    return {"kind": "image", "image": np.asarray(Image.open(os.path.join(up, "image.png")).convert("RGB")), "upstream": True}


def crop(a, b, where):
    """A 512 px crop of each at 100% from the middle, side by side."""
    h, w = a.shape[:2]
    s = min(512, h, w)
    y, x = (h - s) // 2, (w - s) // 2
    Image.fromarray(np.concatenate([a[y:y + s, x:x + s], b[y:y + s, x:x + s]], axis=1)).save(where)


def equalized(frames, source):
    """Upstream frames through our decode's cell equalizer, as `official.decode_chunks` runs it (per 17-frame piece)."""
    import torch

    spec = importlib.util.spec_from_file_location("cell_texture", os.path.join(source, "minimax-h3", "minimax_h3", "cell_texture.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    equalize = module.CellEqualizer(16)
    pieces = []
    for start in range(0, len(frames), 17):
        piece = torch.from_numpy(frames[start:start + 17]).permute(0, 3, 1, 2).float() / 255.0
        pieces.append((equalize(piece).clamp(0, 1) * 255).round().byte().permute(0, 2, 3, 1).numpy())
    return np.concatenate(pieces)


def video_row(a, b, label, where):
    n = min(len(a["frames"]), len(b["frames"]))
    fa, fb = a["frames"][:n], b["frames"][:n]
    if fa.shape != fb.shape:
        return f"{label}: frames {fa.shape} vs {fb.shape}"
    per = [psnr(x, y) for x, y in zip(fa, fb)]
    picks = np.linspace(0, n - 1, 5).round().astype(int)
    d = dists(fa[picks], fb[picks])
    crop(fa[n // 2], fb[n // 2], where)
    return f"{label}: {n} frames, PSNR mean {np.mean(per):.2f} dB (min {min(per):.2f}, first {per[0]:.2f}, last {per[-1]:.2f})" + (
        f", DISTS {d:.4f}" if d is not None else "")


def pairs(out, source, spec):
    os.makedirs(os.path.join(out, "crops"), exist_ok=True)
    for a_name, b_name, what in json.loads(spec):
        try:
            a, b = load(out, a_name), load(out, b_name)
        except Exception as error:
            print(f"{a_name} vs {b_name} ({what}): missing {error}")
            continue
        head = f"{a_name} vs {b_name} ({what})"
        if a["kind"] == "image":
            if a["image"].shape != b["image"].shape:
                print(f"{head}: sizes {a['image'].shape} vs {b['image'].shape}")
                continue
            d = dists(a["image"][None], b["image"][None])
            crop(a["image"], b["image"], os.path.join(out, "crops", f"{a_name}--{b_name}.png"))
            print(f"{head}: PSNR {psnr(a['image'], b['image']):.2f} dB" + (f", DISTS {d:.4f}" if d is not None else ""))
            continue
        rows = [video_row(a, b, head, os.path.join(out, "crops", f"{a_name}--{b_name}.png"))]
        if "latents" in a and "latents" in b:
            la, lb = a["latents"].reshape(a["latents"].shape[-4:]), b["latents"].reshape(b["latents"].shape[-4:])
            if la.shape == lb.shape:
                err = np.linalg.norm((la - lb).ravel()) / np.linalg.norm(lb.ravel())
                frame = [np.linalg.norm((la[:, t] - lb[:, t]).ravel()) / np.linalg.norm(lb[:, t].ravel()) for t in range(la.shape[1])]
                cos = float(np.dot(la.ravel(), lb.ravel()) / np.linalg.norm(la.ravel()) / np.linalg.norm(lb.ravel()))
                rows.append(f"  latents {la.shape}: rel L2 {err:.4f} (per latent frame first {frame[0]:.4f}, max {max(frame):.4f}), cosine {cos:.5f}")
            else:
                rows.append(f"  latents {la.shape} vs {lb.shape}")
        rate = b.get("rate") or a.get("rate") or 32000
        sa = a.get("audio") if "audio" in a else video_audio(a["mp4"], rate) if "mp4" in a else None
        sb = b.get("audio") if "audio" in b else video_audio(b["mp4"], rate) if "mp4" in b else None
        if sa is not None and sb is not None:
            m = min(sa.shape[1], sb.shape[1])
            noise = np.sum((sa[:, :m] - sb[:, :m]) ** 2)
            snr = 99.0 if noise == 0 else 10 * math.log10(np.sum(sb[:, :m] ** 2) / noise)
            rows.append(f"  sound: SNR {snr:.2f} dB over {m / rate:.2f} s, RMS {np.sqrt(np.mean(sa[:, :m] ** 2)):.4f} vs {np.sqrt(np.mean(sb[:, :m] ** 2)):.4f}")
        if source and b.get("upstream") and not a.get("upstream"):
            try:
                eq = dict(b, frames=equalized(b["frames"], source))
                rows.append("  " + video_row(a, eq, "against upstream + our cell equalizer", os.path.join(out, "crops", f"{a_name}--{b_name}-eq.png")))
                floor = x264(eq["frames"], os.path.join(out, "crops", f"{b_name}-eq-crf14.mp4"))
                rows.append("  " + video_row({"frames": floor}, eq, "floor: upstream + equalizer, through our x264 crf 14",
                                             os.path.join(out, "crops", f"{b_name}-floor.png")))
            except Exception as error:
                rows.append(f"  equalizer/floor rows failed: {error!r}")
        print("\n".join(rows), flush=True)


def selftest(where):
    os.makedirs(os.path.join(where, "ours", "a"), exist_ok=True)
    rng = np.random.default_rng(0)
    frames = (rng.random((34, 64, 96, 3)) * 255).astype(np.uint8)
    x264(frames, os.path.join(where, "ours", "a", "a.mp4"))
    up = os.path.join(where, "upstream", "g", "b")
    os.makedirs(up, exist_ok=True)
    np.save(os.path.join(up, "frames.npy"), frames)
    np.save(os.path.join(up, "latents.npy"), rng.standard_normal((1, 24, 2, 4, 6)).astype(np.float32))
    np.save(os.path.join(up, "audio.npy"), rng.standard_normal((2, 32000)).astype(np.float32) * 0.1)
    open(os.path.join(up, "rate.txt"), "w").write("32000")
    os.makedirs(os.path.join(where, "ours", "c"), exist_ok=True)
    Image.fromarray(frames[0]).save(os.path.join(where, "ours", "c", "c.png"))
    os.makedirs(os.path.join(where, "upstream", "g", "d"), exist_ok=True)
    Image.fromarray(frames[1]).save(os.path.join(where, "upstream", "g", "d", "image.png"))
    print(psnr(frames[0], frames[0]), dists(frames[:1], frames[1:2]))
    pairs(where, None, json.dumps([["a", "b", "video"], ["c", "d", "image"]]))


if __name__ == "__main__":
    if sys.argv[1] == "selftest":
        selftest(sys.argv[2])
    else:
        pairs(sys.argv[2], sys.argv[3], sys.argv[4])
