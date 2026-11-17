"""se-163, on CPU: minimax-h3's own planning of an edit (the calls `_render` makes for a plate, in its order) over plates
of every length from 1 frame to 3 minutes, at 24, 25, 29.97, 30 and 60 fps, and its guide reader on clips of those rates.

    uv run --frozen --project <packages>/minimax-h3 python plan.py <out.json>

Checked for every plate:
- each segment samples an H3-valid window: 17n + 5 frames, at most 362; every join carries 22 frames;
- the delivered frames partition the plate: segment i delivers plate slots [start_i, start_i+1), the output has exactly the
  plate's frames (its 24-fps slots, `footage_frames`), nothing skipped, repeated or shifted;
- only the last segment samples past the plate, and those frames are cut (never decoded or delivered);
- each segment's guide window starts on its first sampled frame's plate slot (its carried head included), reads the plate
  frames it has and holds the last for the rest;
- the delivered sound runs end to end over the same span (regenerate, new); keep lays the plate's from 0 to that end.
And for the guide reader (`read_reference_video`, guide=True): every 24-fps slot holds the source frame showing then,
in order, none skipped past a slot, and the tail past the clip holds its last frame.
"""

from __future__ import annotations

import json
import math
import sys
import time
from fractions import Fraction
from itertools import accumulate
from types import SimpleNamespace

import numpy as np
from cozy_runtime.author import Prompt
from cozy_runtime.media import DecodedVideoFrame

from minimax_h3.continuation import Carry
from minimax_h3.official import FPS, MAX_FRAMES, read_reference_video
from minimax_h3.shot import (
    JOIN_S,
    VIDEO_COLOR,
    Beat,
    Shot,
    blocks,
    footage_frames,
    plan_segments,
    prompt_parts,
    segment_plans,
    shot_windows,
    written,
)
from story import StoryReference

RATES = {"24": Fraction(24), "25": Fraction(25), "29.97": Fraction(30000, 1001), "30": Fraction(30), "60": Fraction(60)}
MOST_S = 180
SAMPLE_RATE = 48000


class Footage:
    """A plate as the planner sees it: its header (`VideoFile.probe`), `frames` source frames at `rate`."""

    def __init__(self, frames: int, rate: Fraction) -> None:
        self.duration = Fraction(frames) / rate

    def probe(self) -> SimpleNamespace:
        return SimpleNamespace(duration=self.duration, width=1280, height=720, pixel_aspect_ratio=Fraction(1))


def plan(frames: int, rate: Fraction, beats: list[int]) -> dict[str, object]:
    """`_render`'s planning of an edit of this plate, as it calls it; its invariants checked. Raises on a miss."""
    plate = Footage(frames, rate)
    shot = Shot(beats=[Beat(duration=d, description=Prompt(text="body_swap: the man speaks.")) for d in beats])
    carry = Carry(shot.carried_frames, True)
    script = written(shot, offset=0.0, length=float(plate.probe().duration))
    duration_s = script.duration_s
    timed, _ = blocks(prompt_parts(script.prompt)[1], duration_s)
    segments, opens, _ = plan_segments(duration_s, [block.at for block in timed if block.at > JOIN_S], carried=False,
                                       carry=carry, split=True)  # fmt: skip
    plans = segment_plans(segments, carry, carried=False, plate=footage_frames(plate))
    del segments[len(plans):], opens[len(plans):]
    *starts, total = accumulate((p.delivered_frames for p in plans), initial=0)
    followed = StoryReference(name="Plate", kind="anchor_video", video=plate)
    windows, _ = shot_windows([(followed, 0.0, math.inf)], [(s - p.prefix_frames, p.sample_frames) for s, p in zip(starts, plans)])
    length = footage_frames(plate)
    slots = math.floor(plate.duration * FPS + Fraction(1, 2))
    assert length == slots, (length, slots)
    assert total == length, f"delivered {total} of the plate's {length}"
    seen: list[int] = []
    sound = 0
    entry = Fraction(0)
    for index, (start, p) in enumerate(zip(starts, plans)):
        assert p.sample_frames % 17 == 5 and p.sample_frames <= MAX_FRAMES, ("grid", index, p.sample_frames)
        assert p.prefix_frames == (22 if index else 0), ("carry", index, p.prefix_frames)
        assert index + 1 == len(plans) or p.cut == 0, ("cut before the last", index)
        assert p.sample_frames - p.prefix_frames - p.cut == p.delivered_frames, ("cut", index)
        if index + 1 == len(plans):
            assert start + p.sample_frames - p.prefix_frames - length == p.cut, ("past the plate", index, p.cut)
        window = windows[index].get("Plate")
        assert window is not None, ("unguided", index)
        assert window[0] == start - p.prefix_frames and window[1] == length, ("window", index, window)
        # The guide reads plate slots [window[0], min(window[0] + sample, length)) and holds the last for the rest;
        # the delivered frames are sample frames [prefix, prefix + delivered), the plate's slots [start, start + delivered).
        read = min(window[0] + p.sample_frames, length) - window[0]
        assert read >= p.prefix_frames + p.delivered_frames, ("delivered past the read plate", index)
        seen.extend(range(start, start + p.delivered_frames))
        a, b = p.audio_samples(entry, SAMPLE_RATE)
        sound += b - a
        entry = p.exit_offset
    assert seen == list(range(length)), "a plate frame skipped, repeated or out of order"
    # The sound delivered end to end covers the plate's span, a sample per join at most.
    assert abs(sound - Fraction(length, FPS) * SAMPLE_RATE) <= len(plans), ("sound", sound, length)
    return {"source_frames": frames, "plate_frames": length, "segments": [p.seconds for p in plans],
            "sampled": [p.sample_frames for p in plans], "cut": plans[-1].cut, "starts": starts}


def guide(rate: Fraction, frames: int, window: int) -> dict[str, object]:
    """`read_reference_video` as a guide, on a clip of `frames` source frames at `rate`, each a flat colour naming it,
    read into a `window` of 24-fps slots."""
    width, height = 160, 90

    def event(i: int) -> DecodedVideoFrame:
        rgb = np.empty((height, width, 3), dtype=np.uint8)
        rgb[..., 0], rgb[..., 1], rgb[..., 2] = i % 256, i // 256, 128
        return DecodedVideoFrame(width=width, height=height, rgb=rgb.tobytes(), pts=i, duration=1,
                                 time_base=1 / rate, pixel_aspect_ratio=Fraction(1), **VIDEO_COLOR)  # fmt: skip

    clip = read_reference_video((event(i) for i in range(frames)), frames=window, audio=False, guide=True)
    got = [int(f[0, 0, 0]) + 256 * int(f[0, 0, 1]) for f in clip.frames]
    assert len(got) == window, ("slots", len(got), window)
    # Slot k shows the source frame showing at k/24: frame i lands on slots round(i/rate*24) .. round((i+1)/rate*24) - 1.
    expected = []
    for k in range(window):
        i = max((j for j in range(frames) if math.floor(Fraction(j) / rate * FPS + Fraction(1, 2)) <= k), default=0)
        expected.append(i)
    held = sum(k >= round(Fraction(frames) / rate * FPS) for k in range(window))
    assert got == expected, ("mapping", [(k, g, e) for k, (g, e) in enumerate(zip(got, expected)) if g != e][:5])
    assert all(b >= a for a, b in zip(got, got[1:])), "slots out of order"
    return {"rate": str(rate), "frames": frames, "window": window, "held": held, "last": got[-1]}


def main() -> None:
    began = time.monotonic()
    checked: list[dict[str, object]] = []
    fails: list[str] = []
    grid = sorted({17 * n + 5 for n in range(1, 22)})
    for name, rate in RATES.items():
        for frames in range(1, int(MOST_S * rate) + 1):
            for beats in ([15], [5, 15]):
                try:
                    row = plan(frames, rate, beats)
                except (AssertionError, ValueError) as miss:
                    fails.append(f"{name} fps, {frames} frames, beats {beats}: {miss!r}")
                    continue
                if beats == [15] and (row["plate_frames"] in grid or row["plate_frames"] - 1 in grid or row["plate_frames"] + 1 in grid
                                      or frames % int(30 * rate) == 0 or frames <= 3):
                    checked.append({"rate": name, **row})
    readers = []
    for rate in RATES.values():
        for window in (124, 362):
            for frames in (window * rate / FPS - 1, window * rate / FPS + 1, 40):
                count = max(1, int(frames))
                try:
                    readers.append(guide(rate, count, window))
                except AssertionError as miss:
                    fails.append(f"guide {rate} fps, {count} frames into {window}: {miss!r}")
    total = sum(int(MOST_S * r) for r in RATES.values()) * 2
    summary = {"plates": total, "failures": len(fails), "seconds": round(time.monotonic() - began, 1),
               "examples": checked, "guide_reads": readers, "fails": fails[:50]}
    with open(sys.argv[1], "w") as out:
        json.dump(summary, out, indent=1)
    print(f"{total} plates (1 frame to {MOST_S} s at {', '.join(RATES)} fps, one beat and two), "
          f"{len(readers)} guide reads: {len(fails)} failures, {summary['seconds']} s")
    for line in fails[:20]:
        print("FAIL", line)
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
