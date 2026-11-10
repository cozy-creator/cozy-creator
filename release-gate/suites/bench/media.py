"""A bench cell's output, judged alone and against an output of the same request and seed (frames.py's instruments: layout
and structure, never a pixel diff, since a kernel swap moves pixels without changing the picture).
    media.py alone FILE              what it is: size, frames, contrast, least SSIM of consecutive frames, largest jump, sound
    media.py pair REFERENCE FILE    how far it is from the reference: layout SSIM, each eighth of a video, sound envelope"""
import hashlib
import json
import os
import subprocess
import sys
from typing import Literal, NotRequired, TypedDict

import numpy as np
import numpy.typing as npt

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..'))
import frames  # noqa: E402

Kind = Literal['image', 'video', 'audio']
IMAGE, VIDEO, AUDIO = ('.png', '.webp', '.jpg', '.jpeg'), ('.mp4', '.webm', '.mov'), ('.wav', '.flac', '.mp3', '.ogg')
# A picture or frame under this luma deviation is flat: a failed or non-finite decode reads back one colour.
FLAT = 2.0
# Consecutive frames of one unbroken shot: 0.96-0.98 for H3's 5 s shots; 0.538 where 0.22.1's two-GPU shot broke into
# tiled, shifted copies (run 43975, against 43974 and 42745).
LEAST = 0.85
# The largest change of a frame's structure (brightness removed) between the 1/16-video windows either side of a frame,
# over the video's median change (sp-speed's frame check): good H3 videos <= 4.4 (a beam flash), broken 43973/43975 >= 20.
JUMP = 10.0
# Against the same request and seed from another build, pod or GPU count. Pictures: layout SSIM at 128x128 grey, recorded
# 0.99-1.00 across v1/0.21.1/0.21.2/0.22.1 and three pods, 0.35-0.70 for another seed. Video: mean absolute difference
# (grey 0..1, 168x96) of each eighth of the video, 0.042-0.046 same seed (0.22.1 one GPU against 0.21.1 two GPUs),
# 0.10-0.31 other seeds; and no eighth over RATIO x the first (the broken half leaves the early frames matching).
# Sound: envelope correlation 0.93-0.99 same seed, -0.13 another.
ALIKE, APART, RATIO, ENVELOPE = 0.90, 0.08, 2.5, 0.6
SILENT_DB = -60.0


class Look(TypedDict):
    """What an output is, alone."""
    kind: Kind
    width: NotRequired[int]
    height: NotRequired[int]
    frames: NotRequired[int]
    contrast: NotRequired[float]
    finite: NotRequired[bool]
    least: NotRequired[float]
    jump: NotRequired[float]
    jump_at: NotRequired[int]
    audio_s: NotRequired[float]
    audio_db: NotRequired[float | None]
    audio_finite: NotRequired[bool]


class Pair(TypedDict):
    """How far an output is from another of the same request and seed."""
    kind: Kind
    identical: bool
    ssim: NotRequired[float]
    eighths: NotRequired[list[float]]
    frames: NotRequired[tuple[int, int]]
    envelope: NotRequired[float | None]


def kind(path: str) -> Kind | None:
    ext = os.path.splitext(path)[1].lower()
    return 'image' if ext in IMAGE else 'video' if ext in VIDEO else 'audio' if ext in AUDIO else None


def sound(path: str) -> npt.NDArray[np.float32]:
    """Mono 8 kHz samples, empty when the file has no sound."""
    try:
        return frames.pcm(path)
    except subprocess.CalledProcessError:
        return np.zeros(0, np.float32)


def envelope(x: npt.NDArray[np.float32], window: int = 400) -> npt.NDArray[np.float64]:
    n = len(x) // window
    return np.abs(x[:n * window]).astype(np.float64).reshape(n, window).mean(axis=1) if n else np.zeros(0)


def jump(f: frames.Frames) -> tuple[float, int]:
    """The largest structure change over the median one, and the frame it falls at."""
    g = f - f.mean((1, 2), keepdims=True)
    g /= g.std((1, 2), keepdims=True) + 1e-6
    w = max(4, len(g) // 16)
    change = np.array([np.abs(g[t:t + w].mean(0) - g[t - w:t].mean(0)).mean() for t in range(w, len(g) - w + 1)])
    return round(float(change.max() / max(float(np.median(change)), 1e-9)), 2), int(change.argmax()) + w


def alone(path: str) -> Look:
    k = kind(path)
    if k is None:
        raise ValueError(f'{path}: not a picture, video or sound')
    if k == 'image':
        a = frames.gray(path, 128, 128)[0]
        p = frames.probe(path)
        return {'kind': k, 'width': p['width'], 'height': p['height'], 'contrast': round(float(a.std()), 2), 'finite': bool(np.isfinite(a).all())}
    x = sound(path)
    look: Look = {'kind': k, 'audio_s': round(len(x) / 8000, 2), 'audio_finite': bool(np.isfinite(x).all()),
                  'audio_db': round(float(20 * np.log10(max(float(np.sqrt(np.mean(x**2))), 1e-9))), 1) if len(x) else None}
    if k == 'video':
        p, f = frames.probe(path), frames.gray(path, 192, 108)
        big, at = jump(frames.gray(path, 168, 96))
        look.update({'width': p['width'], 'height': p['height'], 'frames': p['frames'], 'contrast': round(float(f.std(axis=(1, 2)).mean()), 2),
                     'least': round(min(frames.ssim(f[i - 1], f[i]) for i in range(1, len(f))), 3), 'jump': big, 'jump_at': at})
    return look


def digest(path: str) -> str:
    with open(path, 'rb') as f:
        return hashlib.sha256(f.read()).hexdigest()


def same_bytes(a: str, b: str) -> bool:
    return os.path.getsize(a) == os.path.getsize(b) and digest(a) == digest(b)


def pair(reference: str, path: str) -> Pair:
    k = kind(path)
    if k is None:
        raise ValueError(f'{path}: not a picture, video or sound')
    said: Pair = {'kind': k, 'identical': same_bytes(reference, path)}
    if k == 'image':
        said['ssim'] = round(frames.ssim(frames.gray(reference, 128, 128)[0], frames.gray(path, 128, 128)[0]), 3)
        return said
    a, b = envelope(sound(reference)), envelope(sound(path))
    n = min(len(a), len(b))
    said['envelope'] = round(float(np.corrcoef(a[:n], b[:n])[0, 1]), 3) if n > 4 and a[:n].std() and b[:n].std() else None
    if k == 'video':
        one, other = frames.gray(reference, 168, 96) / 255, frames.gray(path, 168, 96) / 255
        said['frames'] = (len(one), len(other))
        if len(one) == len(other):
            said['eighths'] = [round(float(part.mean()), 4) for part in np.array_split(np.abs(one - other).mean((1, 2)), 8)]
    return said


def faults(now: Look, was: Look | None = None) -> list[str]:
    """Why an output is no output: against its own kind, and against the stored reference's shape when there is one."""
    why = []
    if now['kind'] in ('image', 'video'):
        if now.get('contrast', 0.0) < FLAT or now.get('finite') is False:
            why.append(f"flat (contrast {now.get('contrast')}): a failed or non-finite decode")
        if was and (now.get('width'), now.get('height')) != (was.get('width'), was.get('height')):
            why.append(f"{now.get('width')}x{now.get('height')}, reference {was.get('width')}x{was.get('height')}")
    if now['kind'] == 'video':
        floor = min(LEAST, was.get('least', LEAST) - 0.1) if was else LEAST
        if now.get('least', 0.0) < floor:
            why.append(f"breaks between two frames (least SSIM {now.get('least')}, at least {floor:.2f})")
        if now.get('jump', 0.0) >= JUMP:
            why.append(f"its structure jumps at frame {now.get('jump_at')} ({now.get('jump')}x the median change, under {JUMP:.0f}x)")
        if was and now.get('frames') != was.get('frames'):
            why.append(f"{now.get('frames')} frames, reference {was.get('frames')}")
    if now['kind'] in ('video', 'audio') and was and was.get('audio_s'):
        if abs(now.get('audio_s', 0.0) - was.get('audio_s', 0.0)) > 0.1:
            why.append(f"sound {now.get('audio_s')} s, reference {was.get('audio_s')} s")
        elif (now.get('audio_db') or SILENT_DB - 1) < SILENT_DB or not now.get('audio_finite'):
            why.append(f"silent or non-finite sound ({now.get('audio_db')} dB)")
    return why


def differs(said: Pair, what: str = 'the reference') -> list[str]:
    """Why an output is not the picture its reference of the same request and seed is."""
    if said['identical']:
        return []
    why = []
    if 'ssim' in said and said['ssim'] < ALIKE:
        why.append(f"layout SSIM {said['ssim']} against {what} (at least {ALIKE})")
    if 'frames' in said and said['frames'][0] != said['frames'][1]:
        why.append(f"{said['frames'][1]} frames against {what}'s {said['frames'][0]}")
    e = said.get('eighths')
    if e:
        if max(e) > APART:
            why.append(f"differs from {what} by {max(e):.3f} in its worst eighth (at most {APART})")
        elif max(e) > RATIO * e[0] and max(e) > 0.03:
            why.append(f"late frames differ {max(e) / max(e[0], 1e-9):.1f}x more from {what} than its first eighth")
    env = said.get('envelope')
    if env is not None and env < ENVELOPE:
        why.append(f"sound envelope {env} against {what} (at least {ENVELOPE})")
    return why


if __name__ == '__main__':
    print(json.dumps(alone(sys.argv[2]) if sys.argv[1] == 'alone' else pair(sys.argv[2], sys.argv[3])))
