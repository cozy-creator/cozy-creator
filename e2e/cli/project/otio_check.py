"""The project cell's OTIO check (e2e/cli/project.sh, cl-716 A2): read a cut's .otio with OpenTimelineIO's own reader and check it against the edit.
Usage: otio_check.py <edit.otio> <fps> <name>:<start>:<end>:<asset path>[:<retimed from fps>|:still] ...
Checks: one video and one audio track; on each, one clip per entry in order, its source range
<start>..<end> at <fps>, referencing <asset path>, whose file (beside the .otio) has that sha256 as
its name; a retimed clip's metadata names its rate; a still says so and is a gap on the audio track;
the timeline is the sum of the entries long."""
import hashlib
import os
import sys
from collections.abc import Iterator, Mapping, Sequence
from typing import Protocol, TypedDict

import opentimelineio as otio  # type: ignore[import-untyped]  # no stubs or py.typed: typed below by what this reads


class RationalTime(Protocol):
    value: float
    rate: float


class TimeRange(Protocol):
    start_time: RationalTime
    duration: RationalTime

    def end_time_exclusive(self) -> RationalTime: ...


class CozyMetadata(TypedDict, total=False):
    retimed_from_fps: float
    still: bool


class Reference(Protocol):
    target_url: str


class Clip(Protocol):
    name: str
    source_range: TimeRange
    metadata: Mapping[str, CozyMetadata]
    media_reference: Reference


class Track(Protocol):
    kind: str

    def find_clips(self) -> Iterator[Clip]: ...


class Timeline(Protocol):
    name: str
    tracks: Sequence[Track]

    def duration(self) -> RationalTime: ...
    def video_tracks(self) -> list[Track]: ...
    def audio_tracks(self) -> list[Track]: ...


path, fps, specs = sys.argv[1], float(sys.argv[2]), [s.split(":") for s in sys.argv[3:]]
timeline: Timeline = otio.adapters.read_from_file(path)
ok = True


def check(good: bool, said: str) -> None:
    global ok
    ok &= bool(good)
    print(f"  {'PASS' if good else 'FAIL'} {said}")


want = sum(int(s[2]) - int(s[1]) for s in specs)
d = timeline.duration()
print(f"OpenTimelineIO {otio.__version__} read {path}: timeline {timeline.name!r}, {d.value:.0f} frames at {d.rate} fps, "
      f"{len(timeline.video_tracks())} video, {len(timeline.audio_tracks())} audio track(s)")
check(d.rate == fps and d.value == want, f"timeline {d.value:.0f} frames at {d.rate}, want {want} at {fps}")
check(len(timeline.video_tracks()) == 1 and len(timeline.audio_tracks()) == 1, "one video and one audio track")
for track in timeline.tracks:
    clips = list(track.find_clips())
    held = [s for s in specs if track.kind == "Video" or s[4:] != ["still"]]
    check(len(clips) == len(held), f"{track.kind}: {len(clips)} clips, want {len(held)}")
    for clip, spec in zip(clips, held):
        name, start, end, asset = spec[0], int(spec[1]), int(spec[2]), spec[3]
        still = spec[4:] == ["still"]
        retimed = float(spec[4]) if len(spec) > 4 and not still else None
        r = clip.source_range
        meta = dict(clip.metadata).get("cozy", {})
        url = clip.media_reference.target_url
        print(f"    {track.kind} clip {clip.name}: frames {r.start_time.value:.0f}-{r.end_time_exclusive().value:.0f} at "
              f"{r.start_time.rate} -> {url} {dict(meta) or ''}")
        check(clip.name == name and r.start_time.value == start and r.duration.value == end - start
              and r.start_time.rate == fps and r.duration.rate == fps, f"{name} {start}-{end} at {fps}")
        check(url == asset, f"{name} references {asset}")
        file = os.path.join(os.path.dirname(os.path.abspath(path)), url)
        digest = hashlib.sha256(open(file, "rb").read()).hexdigest() if os.path.exists(file) else None
        check(digest is not None and os.path.basename(url).startswith(digest), f"{url} exists, named by its sha256")
        check(meta.get("retimed_from_fps") == retimed, f"{name} retimed_from_fps {meta.get('retimed_from_fps')}, want {retimed}")
        check(bool(meta.get("still")) == still, f"{name} still {meta.get('still')}, want {still}")
print("otio", "PASS" if ok else "FAIL")
