"""A video cell's output contract, including causal-frame duration and its soundtrack clock."""
from __future__ import annotations

import json
from pathlib import Path

from cell import Check, Json


def video(width: int, height: int, frame_count: int, sample_rate: int = 0) -> Check:
    def check(result: Json, files: list[Path]) -> str | None:
        if not isinstance(result, dict):
            return "video result is not an object"
        if (result.get('width'), result.get('height'), result.get('frame_count'), result.get('fps')) != (width, height, frame_count, 24.0):
            return "video result does not report its declared canvas/frame count/24fps"
        if len(files) != 1:
            return "video cell did not produce exactly one media file"
        path = files[0]
        sidecar = path.with_suffix(path.suffix + '.json')
        if path.suffix == '.raw':
            facts = json.loads(sidecar.read_text())
            if facts.get('shape') != [frame_count, height, width, 3] or facts.get('fps') != 24.0:
                return "raw samples do not match the result's geometry and clock"
            channels, rate, samples = facts.get('audio_channels'), facts.get('sample_rate'), facts.get('audio_samples')
            if (channels, rate) != (2 if sample_rate else 0, sample_rate):
                return "raw soundtrack channels/rate do not match the requested audio mode"
            if sample_rate and (not isinstance(samples, int) or abs(samples / sample_rate - frame_count / 24.0) > 1 / 24):
                return "soundtrack and video differ by more than one frame"
        else:
            import av

            with av.open(str(path)) as media:
                stream = media.streams.video[0]
                if (stream.width, stream.height) != (width, height):
                    return "encoded video canvas does not match the result"
                if bool(media.streams.audio) != bool(sample_rate):
                    return "encoded soundtrack presence does not match audio mode"
        return None

    return check
