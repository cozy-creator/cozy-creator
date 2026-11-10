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
# The largest change of local structure (each 16 px block against its own mean and spread) between the 1/16-video windows
# either side of a frame, over the video's median change. On disk (11-10): 62 good videos (v1, 0.21.x, 0.22.1; H3 with and
# without references, capped and not, SeedVR2) 1.06-2.24, 21 more 0.22.1 sp-speed/spmd outputs 1.10-2.23, the two lighthouse
# beam flashes (44006, gaara's 44051) 2.98 and 2.85; 0.22.1's scrambled two-GPU runs 43975, 43973: 7.02 and 11.14. With the
# frame's brightness removed only as a whole (before 11-10) the flashes read 10.0 and 11.5 against 20.5 and 23.5 broken. The
# least SSIM of consecutive frames, reported, does not separate them: 0.70-0.99 good, 0.54 and 0.65 broken.
JUMP = 4.5
# Against the same request and seed from another build, pod or GPU count. Pictures: layout SSIM at 128x128 grey, recorded
# 0.99-1.00 across v1/0.21.1/0.21.2/0.22.1 and three pods, 0.35-0.70 for another seed. Video: mean absolute difference
# (grey 0..1, 168x96) of each eighth of the video. Same-seed one-GPU/two-GPU pairs on disk (11-10: three of released 0.22.1,
# five experimental, across kernel paths: one GPU runs the package's int4 shared-QKV producer, two the Runtime's sparse
# kernel) differ 0.017-0.084 in their worst eighth and drift smoothly: no eighth more than 1.35x the one before. Those pairs
# with the two-GPU side's late half tiled and shifted (43975's frames) reach 0.14-0.27 with a step of 3.9-5.1x; 43975 and
# 43973 against 43974 (another seed) 0.23-0.24. So a video fails over APART in any eighth (a different video) or with a step
# over STEP into an eighth above 0.05 (a break partway). Sound: envelope correlation 0.93-0.99 same seed, -0.13 another.
ALIKE, APART, STEP, ENVELOPE = 0.90, 0.15, 2.0, 0.6
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


def jump(f: frames.Frames, block: int = 16) -> tuple[float, int]:
    """The largest change of local structure over the median one, and the frame it falls at. Each block of each frame is
    held against its own mean and spread first, so light flooding part of the frame (a lighthouse beam into the camera)
    is not a break by itself, while a frame cut into shifted tiles (43975, 43973) is. Changes are of window means."""
    t, h, w = f.shape
    b = f[:, :h // block * block, :w // block * block].reshape(t, h // block, block, w // block, block)
    g = ((b - b.mean((2, 4), keepdims=True)) / (b.std((2, 4), keepdims=True) + 2.0)).reshape(t, h // block * block, -1)
    n = max(4, t // 16)
    change = np.array([np.abs(g[i:i + n].mean(0) - g[i - n:i].mean(0)).mean() for i in range(n, t - n + 1)])
    return round(float(change.max() / max(float(np.median(change)), 1e-9)), 2), int(change.argmax()) + n


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
        if now.get('jump', 0.0) >= JUMP:
            why.append(f"its structure jumps at frame {now.get('jump_at')} ({now.get('jump')}x the median change, at most {JUMP:g}x)")
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
        steps = [(e[i] / max(e[i - 1], 1e-9), i) for i in range(1, len(e)) if e[i] > 0.05]
        if max(e) > APART:
            why.append(f"differs from {what} by {max(e):.3f} in its worst eighth (at most {APART})")
        elif steps and max(steps)[0] > STEP:
            why.append(f"breaks from {what} at eighth {max(steps)[1] + 1}: {max(steps)[0]:.1f}x the eighth before (at most {STEP:g}x)")
    env = said.get('envelope')
    if env is not None and env < ENVELOPE:
        why.append(f"sound envelope {env} against {what} (at least {ENVELOPE})")
    return why


if __name__ == '__main__':
    print(json.dumps(alone(sys.argv[2]) if sys.argv[1] == 'alone' else pair(sys.argv[2], sys.argv[3])))
