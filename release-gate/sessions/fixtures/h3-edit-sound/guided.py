"""se-163's 4090 A/B, on CPU in one arm's locked env: what that arm's own code makes of the Kelly plate (12.6 s at 29.97
fps: 378 source frames, 303 slots on the 24-fps clock) in one 13 s Ref2VA call. Prints JSON: the frames the call samples,
the guide clip `read_reference_video` reads for it (guide=True), the frames the guide's encode keeps (`_snapped`, the
official encode step's 17n + 5) and their latent frames, and the Runtime and TensorFS this env resolved.

    <env>/bin/python -I guided.py
"""

from __future__ import annotations

import json
from fractions import Fraction
from importlib.metadata import version

import numpy as np
from cozy_runtime.media import DecodedVideoFrame
from minimax_h3.continuation import ContinuationPlan, video_tokens
from minimax_h3.official import _snapped, read_reference_video
from minimax_h3.shot import VIDEO_COLOR

RATE, SOURCE, SECONDS = Fraction(30000, 1001), 378, 13


def event(i: int) -> DecodedVideoFrame:
    rgb = np.full((90, 160, 3), i % 256, dtype=np.uint8)
    return DecodedVideoFrame(width=160, height=90, rgb=rgb.tobytes(), pts=i, duration=1, time_base=1 / RATE,
                             pixel_aspect_ratio=Fraction(1), **VIDEO_COLOR)  # fmt: skip


sampled = ContinuationPlan(SECONDS).sample_frames
read = len(read_reference_video((event(i) for i in range(SOURCE)), frames=sampled, audio=False, guide=True).frames)
guided = _snapped(read)
print(json.dumps({"sampled": sampled, "sampled_latents": video_tokens(sampled), "guide_read": read, "guided": guided,
                  "guided_latents": video_tokens(guided), "runtime": version("cozy-runtime"), "tensorfs": version("tensorfs"),
                  "minimax_h3": version("minimax-h3")}))
