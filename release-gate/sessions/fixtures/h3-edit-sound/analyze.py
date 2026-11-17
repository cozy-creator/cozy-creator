"""se-163: what each edit made, against its plate. `analyze.py <manifest.json> <out dir>`; the manifest lists each video
{name, path, plate, mode (keep|regenerate|new|before|control), seams, long}. Writes <out>/analysis.json and contact sheets.

- frames: the video's frame count against the plate's 24-fps slots;
- keep: the plate's PCM from its first frame and the video's from its first, sample for sample, and the audio packets;
- lip sync: mouth opening (MediaPipe Face Mesh, inner lips over face height) against the sound's loudness, 24 a second,
  correlated at lags of -10..10 frames: the best lag and its r; the plate with its own sound and shifted 0.5 s are the
  controls;
- the end: the last 48 frames' motion (frame-to-frame change) and mouth against the plate's;
- seams: each join's frame change over its neighbours' median, beside the plate's own there;
- voice: Whisper's words, median pitch and MFCC likeness against the plate's.
"""

from __future__ import annotations

import hashlib
import json
import math
import subprocess
import sys
from fractions import Fraction
from pathlib import Path

import av
import numpy as np

FPS = 24
LAGS = range(-10, 11)


def slots(path: str) -> float:
    with av.open(path) as c:
        v = c.streams.video[0]
        return float(c.duration / 1e6) if v.duration is None else float(v.duration * v.time_base)


def pictures(path: str, width: int = 640, count: int | None = None) -> tuple[list[np.ndarray], float]:
    """The video on the 24-fps clock from its first frame (each slot the frame showing then), RGB at `width`, and its
    frame rate."""
    out: list[np.ndarray] = []
    with av.open(path) as c:
        v = c.streams.video[0]
        rate = float(v.average_rate or 24)
        origin = None
        held = None
        for frame in c.decode(v):
            t = float(frame.pts * frame.time_base)
            origin = t if origin is None else origin
            while held is not None and len(out) < round((t - origin) * FPS):
                out.append(held)
            held = frame.to_ndarray(format="rgb24", width=width, height=round(width * frame.height / frame.width / 2) * 2)
        if held is not None:
            out.append(held)
    return (out if count is None else out[:count]), rate


def frame_count(path: str) -> int:
    with av.open(path) as c:
        return sum(1 for _ in c.demux(c.streams.video[0]) if _.size)


def pcm(path: str) -> tuple[np.ndarray, int]:
    """`(channels, samples)` float32 from the video's first frame on (its first frame's time is sample 0)."""
    with av.open(path) as c:
        v = c.streams.video[0]
        origin = Fraction(v.start_time or 0) * Fraction(v.time_base.numerator, v.time_base.denominator)
        a = c.streams.audio[0]
        rate = a.codec_context.sample_rate
        parts, first = [], None
        for frame in c.decode(a):
            data = frame.to_ndarray()
            data = data if frame.format.is_planar else data.reshape(-1, frame.layout.nb_channels).T
            if first is None:
                first = round((Fraction(frame.pts) * Fraction(frame.time_base.numerator, frame.time_base.denominator) - origin) * rate)
            parts.append(data.astype(np.float32))
    wave = np.concatenate(parts, axis=1)
    if first and first > 0:
        wave = np.concatenate([np.zeros((wave.shape[0], first), np.float32), wave], axis=1)
    elif first and first < 0:
        wave = wave[:, -first:]
    return wave, rate


def packets(path: str, until: float) -> list[str]:
    with av.open(path) as c:
        a = c.streams.audio[0]
        tb = float(a.time_base)
        return [hashlib.sha256(bytes(p)).hexdigest()[:16] for p in c.demux(a) if p.size and p.pts is not None and p.pts * tb < until]


def envelope(wave: np.ndarray, rate: int, count: int) -> np.ndarray:
    mono = wave.mean(axis=0)
    step = rate / FPS
    rms = [np.sqrt(np.mean(mono[int(k * step) : int((k + 1) * step)] ** 2) + 1e-12) for k in range(count)]
    return np.log(np.array(rms) + 1e-4)


def mouths(frames: list[np.ndarray]) -> np.ndarray:
    import mediapipe as mp

    found = []
    with mp.solutions.face_mesh.FaceMesh(static_image_mode=False, max_num_faces=1, min_detection_confidence=0.3) as mesh:
        for f in frames:
            r = mesh.process(f)
            if not r.multi_face_landmarks:
                found.append(np.nan)
                continue
            p = r.multi_face_landmarks[0].landmark
            found.append(abs(p[13].y - p[14].y) / max(1e-6, abs(p[10].y - p[152].y)))
    m = np.array(found)
    if np.isnan(m).all():
        return m
    idx = np.arange(len(m))
    m[np.isnan(m)] = np.interp(idx[np.isnan(m)], idx[~np.isnan(m)], m[~np.isnan(m)])
    return m


def z(x: np.ndarray) -> np.ndarray:
    return (x - x.mean()) / (x.std() + 1e-9)


def sync(mouth: np.ndarray, loud: np.ndarray) -> dict[str, float]:
    n = min(len(mouth), len(loud))
    m, a = z(mouth[:n]), z(loud[:n])
    scores = {}
    for lag in LAGS:
        x, y = (m[: n - lag], a[lag:]) if lag >= 0 else (m[-lag:], a[: n + lag])
        scores[lag] = float(np.corrcoef(x, y)[0, 1])
    best = max(scores, key=scores.get)
    return {"r_at_0": round(scores[0], 3), "best_lag_frames": best, "best_r": round(scores[best], 3),
            "detected": round(float(np.mean(~np.isnan(mouth))), 3)}


def changes(frames: list[np.ndarray]) -> np.ndarray:
    gray = [f[::4, ::4].mean(axis=2) for f in frames]
    return np.array([0.0] + [float(np.abs(b - a).mean()) for a, b in zip(gray, gray[1:])])


def sheet(name: str, out: Path, rows: list[list[np.ndarray]]) -> str:
    from PIL import Image

    h = max(f.shape[0] for row in rows for f in row)
    w = max(f.shape[1] for row in rows for f in row)
    cols = max(len(row) for row in rows)
    canvas = Image.new("RGB", (cols * w, len(rows) * h))
    for r, row in enumerate(rows):
        for c, f in enumerate(row):
            canvas.paste(Image.fromarray(f), (c * w, r * h))
    path = out / f"{name}.jpg"
    canvas.save(path, quality=85)
    return path.name


def voice(wave: np.ndarray, rate: int, model: object) -> dict[str, object]:
    import librosa

    mono = librosa.resample(wave.mean(axis=0), orig_sr=rate, target_sr=16000)
    f0, voiced, _ = librosa.pyin(mono, fmin=60, fmax=400, sr=16000, frame_length=1024)
    mfcc = librosa.feature.mfcc(y=mono, sr=16000, n_mfcc=20).mean(axis=1)
    segments, _ = model.transcribe(mono.astype(np.float32), language="en", beam_size=1)  # type: ignore[attr-defined]
    return {"words": " ".join(s.text.strip() for s in segments), "f0_median_hz": round(float(np.nanmedian(f0[voiced])), 1)
            if voiced.any() else None, "mfcc": mfcc.tolist()}


def wer(ref: str, hyp: str) -> float:
    r, h = ref.lower().split(), hyp.lower().split()
    d = list(range(len(h) + 1))
    for i in range(1, len(r) + 1):
        prev, d[0] = d[0], i
        for j in range(1, len(h) + 1):
            prev, d[j] = d[j], min(d[j] + 1, d[j - 1] + 1, prev + (r[i - 1] != h[j - 1]))
    return round(d[len(h)] / max(1, len(r)), 3)


def main() -> None:
    manifest = json.loads(Path(sys.argv[1]).read_text())
    out = Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    report: dict[str, object] = {}
    from faster_whisper import WhisperModel

    whisper = WhisperModel("tiny.en", device="cpu", compute_type="int8")
    plates: dict[str, dict[str, object]] = {}
    for item in manifest:
        name, path, plate, mode = item["name"], item["path"], item["plate"], item["mode"]
        row: dict[str, object] = {"mode": mode}
        plate_slots = math.floor(slots(plate) * FPS + 0.5)
        made = frame_count(path)
        row["frames"], row["plate_slots"] = made, plate_slots
        row["frames_equal_plate"] = made == plate_slots
        long = bool(item.get("long"))
        if plate not in plates:
            frames, rate = pictures(plate, 320 if long else 640)
            wave, sr = pcm(plate)
            plates[plate] = {"frames": frames, "rate": rate, "wave": wave, "sr": sr,
                             "mouth": None if long else mouths(frames), "changes": changes(frames)}
            if not long:
                p = plates[plate]
                loud = envelope(wave, sr, len(frames))
                report[f"control {Path(plate).name}"] = {
                    "own sound": sync(p["mouth"], loud),
                    "sound 0.5 s late": sync(p["mouth"], np.concatenate([np.full(12, loud.min()), loud])[: len(loud)]),
                    "voice": {k: v for k, v in voice(wave[:, : int(len(frames) / FPS * sr)], sr, whisper).items()},
                }
        p = plates[plate]
        frames, _ = pictures(path, 320 if long else 640)
        wave, sr = pcm(path)
        n = min(wave.shape[1], int(Fraction(made, FPS) * sr))
        row["sound_seconds"] = round(wave.shape[1] / sr, 4)
        row["video_seconds"] = round(made / FPS, 4)
        if mode == "keep":
            ref = p["wave"][:, :n]
            row["keep_pcm_sha256"] = hashlib.sha256(np.ascontiguousarray(wave[:, :n]).tobytes()).hexdigest()
            row["plate_pcm_sha256"] = hashlib.sha256(np.ascontiguousarray(ref).tobytes()).hexdigest()
            row["keep_pcm_identical"] = row["keep_pcm_sha256"] == row["plate_pcm_sha256"] and sr == p["sr"]
            row["keep_samples_compared"] = n
            row["keep_max_abs_diff"] = float(np.abs(wave[:, :n] - ref).max()) if ref.shape == wave[:, :n].shape else None
            mine, theirs = packets(path, made / FPS), packets(plate, made / FPS)
            row["audio_packets_equal"] = mine == theirs[: len(mine)] or mine[1:] == theirs[: len(mine) - 1] or mine == theirs[1 : len(mine) + 1]
        if not long:
            mouth = mouths(frames)
            row["lip_sync"] = sync(mouth, envelope(wave, sr, len(frames)))
            if mode == "new":
                row["lip_sync_with_plate_sound"] = sync(mouth, envelope(p["wave"], p["sr"], len(frames)))
            k = min(len(frames), len(p["frames"]))
            tail = slice(max(0, k - 48), k)
            ch = changes(frames)
            row["end_motion_r"] = round(float(np.corrcoef(ch[tail][1:], p["changes"][tail][1:])[0, 1]), 3)
            row["end_mouth_r"] = round(float(np.corrcoef(mouth[tail], p["mouth"][tail])[0, 1]), 3)
            row["sheet_end"] = sheet(f"{name}-end", out, [frames[k - 24 : k : 4], p["frames"][k - 24 : k : 4]])
            if len(frames) > len(p["frames"]):
                row["frames_past_plate"] = len(frames) - len(p["frames"])
                row["sheet_past"] = sheet(f"{name}-past", out, [frames[len(p['frames']) - 4 :: 4][:8]])
            if mode in ("regenerate", "new", "before"):
                said = voice(wave[:, :n], sr, whisper)
                theirs = report[f"control {Path(plate).name}"]["voice"]
                a, b = np.array(said.pop("mfcc")), np.array(theirs["mfcc"])
                row["voice"] = {**said, "wer_vs_plate": wer(theirs["words"], said["words"]),
                                "mfcc_cosine_vs_plate": round(float(a @ b / np.linalg.norm(a) / np.linalg.norm(b)), 3)}
        else:
            ch = changes(frames)
            seams = []
            for s in item.get("seams", []):
                if 12 <= s < len(ch) - 12 and s < len(p["changes"]) - 12:
                    near = np.r_[ch[s - 12 : s], ch[s + 1 : s + 13]]
                    pnear = np.r_[p["changes"][s - 12 : s], p["changes"][s + 1 : s + 13]]
                    seams.append({"frame": s, "change": round(float(ch[s]), 2), "over_neighbours": round(float(ch[s] / np.median(near)), 2),
                                  "plate_over_neighbours": round(float(p["changes"][s] / np.median(pnear)), 2)})
                    sheet(f"{name}-seam-{s}", out, [frames[s - 3 : s + 3], p["frames"][s - 3 : s + 3]])
            row["seams"] = seams
            bounds = [0, *item.get("seams", []), len(frames)]
            row["sheet_segments"] = sheet(f"{name}-segments", out, [[frames[(a + b) // 2] for a, b in zip(bounds, bounds[1:])]])
            k = min(len(frames), len(p["frames"]))
            tail = slice(max(0, k - 48), k)
            row["end_motion_r"] = round(float(np.corrcoef(ch[tail][1:], p["changes"][tail][1:])[0, 1]), 3)
            row["sheet_end"] = sheet(f"{name}-end", out, [frames[k - 24 : k : 4], p["frames"][k - 24 : k : 4]])
        report[name] = row
        print(name, json.dumps({k: v for k, v in row.items() if not k.startswith("sheet")}), flush=True)
    (out / "analysis.json").write_text(json.dumps(report, indent=1, default=str))


if __name__ == "__main__":
    main()
