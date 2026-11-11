"""Compare retained H3 rank-pair media with one decoder; run no product or model code."""

import argparse
import hashlib
import json
from pathlib import Path
import platform
import re

import av
import numpy as np


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def stamp(frame) -> dict[str, object]:
    return {"pts": frame.pts, "time_base": str(frame.time_base),
            "time": str(frame.pts * frame.time_base) if frame.pts is not None else None}


def media(path: Path):
    pixels, frames = [], []
    with av.open(str(path)) as container:
        stream = container.streams.video[0]
        stream.codec_context.thread_count = 1
        codec = stream.codec_context.name
        for frame in container.decode(video=0):
            rgb = np.ascontiguousarray(frame.to_ndarray(format="rgb24"))
            pixels.append(rgb)
            frames.append({**stamp(frame), "width": frame.width, "height": frame.height,
                           "rgb24_sha256": sha(rgb.tobytes())})
    if not frames:
        raise ValueError(f"no video frames: {path}")
    audio, audio_pts = [], []
    with av.open(str(path)) as container:
        if container.streams.audio:
            container.streams.audio[0].codec_context.thread_count = 1
            resampler = av.AudioResampler(format="fltp", layout="stereo", rate=48000)
            for frame in container.decode(audio=0):
                audio_pts.append(stamp(frame))
                audio.extend(part.to_ndarray() for part in resampler.resample(frame))
            audio.extend(part.to_ndarray() for part in resampler.resample(None))
    pcm = np.ascontiguousarray(np.concatenate(audio, axis=1), dtype="<f4") if audio else None
    return ({"path": str(path), "file_sha256": sha(path.read_bytes()), "video_codec": codec,
             "frames": frames, "audio": None if pcm is None else {
                 "format": "stereo planar float32 little-endian", "sample_rate": 48000,
                 "samples_per_channel": pcm.shape[1], "pcm_sha256": sha(pcm.tobytes()),
                 "source_frames": audio_pts}}, pixels, pcm)


def difference(left, right) -> dict[str, object]:
    lmeta, lframes, laudio = left
    rmeta, rframes, raudio = right
    value: dict[str, object] = {"frame_counts": [len(lframes), len(rframes)],
                              "pts_equal": [f["time"] for f in lmeta["frames"]] == [f["time"] for f in rmeta["frames"]]}
    if len(lframes) != len(rframes) or any(a.shape != b.shape for a, b in zip(lframes, rframes)):
        value["video_error"] = "frame count or dimensions differ"
    else:
        rows = []
        for index, (a, b) in enumerate(zip(lframes, rframes)):
            delta = np.abs(a.astype(np.int16) - b.astype(np.int16))
            rows.append({"frame": index, "mean_abs_255": float(delta.mean()),
                         "max_abs_255": int(delta.max()), "rgb_equal": bool(np.array_equal(a, b))})
        worst = max(rows, key=lambda row: row["mean_abs_255"])
        value.update(rgb_equal=all(row["rgb_equal"] for row in rows), worst=worst, per_frame=rows)
    if laudio is None or raudio is None:
        value["audio"] = {"present": [laudio is not None, raudio is not None]}
    elif laudio.shape != raudio.shape:
        value["audio"] = {"sample_shapes": [list(laudio.shape), list(raudio.shape)]}
    else:
        delta = np.abs(laudio.astype(np.float64) - raudio.astype(np.float64))
        value["audio"] = {"pcm_equal": bool(np.array_equal(laudio, raudio)),
                          "mean_abs": float(delta.mean()), "max_abs": float(delta.max())}
    return value


def decoder() -> dict[str, object]:
    root = Path(av.__file__).resolve().parent
    native = [*root.rglob("*.so"), *(root.parent / "av.libs").glob("*.so*")]
    return {"python": platform.python_version(), "av": av.__version__, "numpy": np.__version__,
            "ffmpeg_libraries": av.library_versions, "video_decoder_threads": 1,
            "native_sha256": {str(path.relative_to(root.parent)): sha(path.read_bytes()) for path in sorted(native)}}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("runs", nargs="+")
    args = parser.parse_args()
    if any(re.fullmatch(r"[0-9]+", run) is None for run in args.runs):
        parser.error("run IDs must be decimal integers")
    results: dict[str, object] = {}
    report: dict[str, object] = {"decoder": decoder(), "runs": results}
    for run in args.runs:
        root = args.artifacts / run / "minimax-h3"
        pair = ("regenerate_shot-native", "regenerate_shot-native-2-ranks")
        result = {}
        for kind in ("input", "output"):
            paths = [root / "inputs" / name / "video.mp4" if kind == "input"
                     else root / "spool" / name / "video-0001" for name in pair]
            left, right = (media(path) for path in paths)
            found = difference(left, right)
            result[kind] = {"native": left[0], "two_ranks": right[0], "difference": found}
            print(f"{run} {kind}: RGB equal={found.get('rgb_equal')}; worst={found.get('worst', found.get('video_error'))}")
        results[run] = result
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
