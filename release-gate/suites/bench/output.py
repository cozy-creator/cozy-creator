"""A bench cell's output, judged alone and against a reference output of the same request and seed (release-gate/frames.py's
instruments: layout, never a pixel diff, since a kernel swap moves pixels without changing the picture).
    output.py alone FILE             what it is: size, frames, contrast, the least SSIM of consecutive frames, its sound
    output.py pair REFERENCE FILE    how far it is from the reference: layout SSIM (each half of a video), sound envelope
`faults` and `differs` turn those into the reasons a cell fails."""
import json
import os
import subprocess
import sys

import numpy as np

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..'))
import frames  # noqa: E402

IMAGE, VIDEO, AUDIO = ('.png', '.webp', '.jpg', '.jpeg'), ('.mp4', '.webm', '.mov'), ('.wav', '.flac', '.mp3', '.ogg')
# A picture or frame under this luma deviation is flat: a failed or non-finite decode reads back one colour.
FLAT = 2.0
# Consecutive frames of one unbroken shot: 0.96-0.98 for H3's 5 s shots, 0.54 where 0.22.1's two-GPU shot broke into
# tiled, shifted copies (run 43975 against 43974 and 42745).
LEAST = 0.85
# Layout SSIM against the same request and seed from another build, pod or GPU count (128x128 grey for a picture, 192x108
# for each half of a video). Recorded pairs, 11-07..09 (v1, 0.21.1, 0.21.2 and 0.22.1 builds on kesshouban, crystal and
# canute): pictures 0.99-1.00 against 0.35-0.70 for another seed of the prompt; H3 halves 0.41-0.55 against 0.19-0.23;
# its sound's envelope 0.93-0.99 against -0.13.
ALIKE = {'image': 0.90, 'video': 0.33}
SILENT_DB, ENVELOPE = -60.0, 0.6


def kind(path):
    ext = os.path.splitext(path)[1].lower()
    return 'image' if ext in IMAGE else 'video' if ext in VIDEO else 'audio' if ext in AUDIO else None


def sound(path):
    """Mono 8 kHz samples, empty when the file has no sound."""
    try:
        return frames.pcm(path)
    except subprocess.CalledProcessError:
        return np.zeros(0, np.float32)


def envelope(x, window=400):
    n = len(x) // window
    return np.abs(x[:n * window]).reshape(n, window).mean(axis=1) if n else np.zeros(0)


def alone(path):
    k = kind(path)
    if k == 'image':
        a = frames.gray(path, 128, 128)[0]
        return {'kind': k, **{key: v for key, v in frames.probe(path).items() if key != 'frames'}, 'contrast': round(float(a.std()), 2),
                'finite': bool(np.isfinite(a).all())}
    x = sound(path)
    said = {'audio_s': round(len(x) / 8000, 2), 'audio_db': round(float(20 * np.log10(max(np.sqrt(np.mean(x**2)), 1e-9))), 1) if len(x) else None,
            'audio_finite': bool(np.isfinite(x).all())}
    if k == 'video':
        return {'kind': k, **frames.coherent(path), **said}
    return {'kind': k, **said}


def pair(reference, path):
    k = kind(path)
    if k == 'image':
        return {'ssim': round(frames.ssim(frames.gray(reference, 128, 128)[0], frames.gray(path, 128, 128)[0]), 3)}
    a, b = envelope(sound(reference)), envelope(sound(path))
    n = min(len(a), len(b))
    said = {'envelope': round(float(np.corrcoef(a[:n], b[:n])[0, 1]), 3) if n > 4 and a[:n].std() and b[:n].std() else None}
    if k == 'video':
        v = frames.variant(reference, path)
        return {**{key: v[key] for key in ('ssim', 'ssim_first', 'ssim_second')}, **said}
    return said


def faults(now, was=None):
    """Why an output is no output: against its own kind, and against the stored reference's shape when there is one."""
    why = []
    if now['kind'] in ('image', 'video'):
        if now['contrast'] < FLAT or now.get('finite') is False:
            why.append(f"flat (contrast {now['contrast']}): a failed or non-finite decode")
        if was and (now['width'], now['height']) != (was['width'], was['height']):
            why.append(f"{now['width']}x{now['height']}, reference {was['width']}x{was['height']}")
    if now['kind'] == 'video':
        floor = min(LEAST, was['least'] - 0.1) if was else LEAST
        if now['least'] < floor:
            why.append(f"breaks between two frames (SSIM {now['least']}, at least {floor:.2f})")
        if was and now['frames'] != was['frames']:
            why.append(f"{now['frames']} frames, reference {was['frames']}")
    if now['kind'] in ('video', 'audio') and (was or {}).get('audio_s'):
        if abs(now['audio_s'] - was['audio_s']) > 0.1:
            why.append(f"sound {now['audio_s']} s, reference {was['audio_s']} s")
        elif now['audio_db'] is None or now['audio_db'] < SILENT_DB or not now['audio_finite']:
            why.append(f"silent or non-finite sound ({now['audio_db']} dB)")
    return why


def differs(kind_, said, what='the reference'):
    """Why an output is not the picture its reference is."""
    why = []
    for key in ('ssim',) if kind_ == 'image' else ('ssim_first', 'ssim_second') if kind_ == 'video' else ():
        if said[key] < ALIKE[kind_]:
            why.append(f"{key} {said[key]} against {what} (at least {ALIKE[kind_]})")
    if said.get('envelope') is not None and said['envelope'] < ENVELOPE:
        why.append(f"sound envelope {said['envelope']} against {what} (at least {ENVELOPE})")
    return why


if __name__ == '__main__':
    print(json.dumps(alone(sys.argv[2]) if sys.argv[1] == 'alone' else pair(sys.argv[2], sys.argv[3])))
