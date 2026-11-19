# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19", "numpy>=1.26"]
# ///
"""MiniMax H3's shot's stand-in (cl-907) for web/segments.js and the web suite's video cell, weightless: a segment for each of its beats in
turn, each its beat's length of its own colour and tone, and each after the first `wait` seconds in counted steps. Its plan is logged as H3
logs it (`h3 shot segments`), which is what Calcifer lays a run's progress on. Each shows as it lands joined after those before it, as
minimax-h3 0.11.1 publishes them (`Segment 1 of 3`, `Segments 1-2 of 3`), and the last is the whole video, the take. Its take says where each
segment after the first starts (`seams`, se-130), and it takes a pickup, as H3 does, so its segments offer Redo (web/video.spec.js); it
renders every segment again."""

import time
from collections.abc import Iterator, Sequence
from fractions import Fraction
from typing import Annotated

import msgspec
import numpy as np
from cozy_runtime.author import App, Context, Outputs, Telemetry, call
from cozy_runtime.fields import Audio, Image, Pickup, Prompt, Range, Total, Video
from cozy_runtime.media import (DecodedAudioChunk, DecodedAudioFormat, DecodedMediaEvent, DecodedMediaHeader, DecodedVideoFormat, DecodedVideoFrame,
                                VideoFile)

app = App()
FPS = 24
RATE = 48000
SIZE = (160, 90)
COLOR = {"color_primaries": 1, "color_transfer": 1, "color_matrix": 1, "color_range": 1}
SOUND = DecodedAudioFormat(channels=2, sample_rate=RATE, channel_layout="stereo", channel_names=("FL", "FR"), time_base=Fraction(1, RATE))
HUES = [(200, 40, 40), (40, 160, 60), (40, 70, 200), (200, 170, 40)]


class Beat(msgspec.Struct, kw_only=True):
    description: Prompt[Image | Video | Audio] = Prompt()
    duration: Annotated[float, Range(1, 15, step=1, unit="s"), Total()] = 10


class Segments(msgspec.Struct, kw_only=True):
    summary: Prompt[Image | Video | Audio] = Prompt()
    beats: list[Beat] = []
    wait: Annotated[float, Range(0, 120)] = 30
    pickup: Pickup | None = None


class Take(msgspec.Struct):
    video: VideoFile
    #: Where each segment after the first starts, in frames (se-130).
    seams: list[int] = []


def _tone(index: int, start: int, count: int) -> DecodedAudioChunk:
    """Segment `index`'s tone, `count` samples of it from `start`, at `start` on the clock."""
    tone = (0.2 * np.sin(2 * np.pi * 220 * (index + 1) * np.arange(start, start + count) / RATE)).astype(np.float32)
    return DecodedAudioChunk(channels=2, sample_count=count, sample_rate=RATE, channel_layout="stereo", channel_names=("FL", "FR"),
                             pcm_f32le=(tone.tobytes(), tone.tobytes()), pts=start, time_base=Fraction(1, RATE))  # fmt: skip


def _clip(index: int, seconds: int) -> Iterator[DecodedMediaHeader | DecodedVideoFrame | DecodedAudioChunk]:
    """`seconds` of segment `index`'s colour with a bar crossing it, under its tone."""
    width, height = SIZE
    yield DecodedMediaHeader(video=DecodedVideoFormat(width=width, height=height, time_base=Fraction(1, FPS), pixel_aspect_ratio=Fraction(1),
                                                      nominal_frame_rate=Fraction(FPS), **COLOR), audio=SOUND)  # fmt: skip
    for second in range(seconds):
        for frame in range(second * FPS, (second + 1) * FPS):
            picture = np.full((height, width, 3), HUES[index % len(HUES)], dtype=np.uint8)
            x = frame * 4 % width
            picture[:, x : x + 6] = 255
            yield DecodedVideoFrame(width=width, height=height, rgb=picture.tobytes(), pts=frame, duration=1, time_base=Fraction(1, FPS),
                                    pixel_aspect_ratio=Fraction(1), **COLOR)  # fmt: skip
        yield _tone(index, second * RATE, RATE)


@app.tool(label="Segments", named="segments")
def segments(ctx: Context, payload: Segments, out: Outputs, tel: Telemetry) -> Take:
    """A shot of a segment a beat, each shown as it lands after those before it, as H3's."""
    joined = out.join_video(SOUND)
    lengths = [int(beat.duration) for beat in payload.beats]
    count, steps, samples = len(lengths), 8, 0
    if not count:
        raise AssertionError("a shot has at least one segment")
    starts = [sum(lengths[:index]) for index in range(count)]
    tel.log("h3 shot segments", segments=count, seconds=",".join(map(str, lengths)), resumed_at=0,
            boundaries_s=",".join(f"{start:.2f}" for start in starts[1:]), delivered_frames=sum(lengths) * FPS)  # fmt: skip
    for index in range(count):
        with tel.scope(f"Segment {index + 1} of {count}"), tel.stage("denoise"):
            for at in range(steps):
                if index:
                    time.sleep(payload.wait / steps)
                ctx.progress(at + 1, steps)
            clip = out.save_video_stream(_clip(index, lengths[index])).video

        def sound(frames: int, index: int = index) -> Iterator[DecodedAudioChunk]:
            nonlocal samples
            yield _tone(index, samples, frames * RATE // FPS)
            samples += frames * RATE // FPS

        last = index + 1 == count
        so_far = joined.append(clip, sound, last=last)
        if last:
            video = joined.finish().video
            out.publish("video", video, label="Video")
            return Take(video, [start * FPS for start in starts[1:]])
        out.publish("video", so_far, label=f"Segments 1-{index + 1} of {count}" if index else f"Segment 1 of {count}")
    raise AssertionError("a shot has at least one segment")


class Joins(msgspec.Struct, kw_only=True):
    seconds: tuple[int, ...] = (2, 3, 2)


class Joined(msgspec.Struct):
    """The same clips, each its own `save_video` (x264's B-frames and all), joined by `save_video_concat` and by `join_video`
    (cl-1210), each under one tone a clip."""

    concat: VideoFile
    joined: VideoFile
    pieces: list[VideoFile]


def _pictures(index: int, seconds: int) -> np.ndarray:
    """`seconds` of clip `index`'s colour with its bar crossing it, as `_clip` draws them."""
    width, height = SIZE
    frames = np.empty((seconds * FPS, height, width, 3), dtype=np.uint8)
    frames[:] = HUES[index % len(HUES)]
    for frame in range(seconds * FPS):
        x = frame * 4 % width
        frames[frame, :, x : x + 6] = 255
    return frames


class Clip(msgspec.Struct, kw_only=True):
    index: int
    seconds: int


class Clipped(msgspec.Struct):
    video: VideoFile


@app.entrypoint(private=True, accelerator=False)
def clip(payload: Clip, out: Outputs) -> Clipped:
    """One clip, its own `save_video`: a child's result is a granted input of its parent, as each H3 segment's is."""
    return Clipped(out.save_video(_pictures(payload.index, payload.seconds), fps=FPS))


@app.entrypoint(accelerator=False)
async def joins(ctx: Context, payload: Joins, out: Outputs, tel: Telemetry) -> Joined:
    """Clips with B-frames, joined both ways: their pictures and each clip's tone must come out where they went in."""
    pieces = [(await call(clip, Clip(index=index, seconds=seconds))).video for index, seconds in enumerate(payload.seconds)]

    def tones(frames: Sequence[int]) -> Iterator[DecodedMediaEvent]:
        yield DecodedMediaHeader(video=None, audio=SOUND)
        start = 0
        for index, count in enumerate(frames):
            yield _tone(index, start, count * RATE // FPS)
            start += count * RATE // FPS

    concat = out.save_video_concat(pieces, tones).video
    join, samples = out.join_video(SOUND), 0
    for index, piece in enumerate(pieces):

        def stretch(frames: int, index: int = index) -> Iterator[DecodedAudioChunk]:
            nonlocal samples
            yield _tone(index, samples, frames * RATE // FPS)
            samples += frames * RATE // FPS

        join.append(piece, stretch, last=index + 1 == len(pieces))
    return Joined(concat, join.finish().video, pieces)
