# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.37", "msgspec>=0.19"]
# ///
"""The project cell's shot renderers: weightless, and each says what it was given. `shoot` makes its
video from its plate, `pieces` too, with a shortest piece of 5 s, and `bare`, which takes no plate and
cannot continue, from its first reference. A shot longer than the cell's plates comes back at their
length, and says so."""

import hashlib
import json
import os
from typing import Annotated

import msgspec
from cozy_runtime.author import App, AudioReference, File, Outputs, ShotRequest, VideoReference

app = App()
PLATE_S = 6


class Pieces(ShotRequest, kw_only=True):
    duration_s: Annotated[float, msgspec.Meta(gt=0, extra={"min_piece_s": 5})]


class Bare(msgspec.Struct, kw_only=True):
    prompt: str
    duration_s: float
    references: list[VideoReference | AudioReference] = msgspec.field(default_factory=list)


class Take(msgspec.Struct):
    video: File
    given: File
    warnings: list[str]


def reference(ref) -> dict:
    file = next(getattr(ref, field) for field in ("video", "audio", "image") if hasattr(ref, field))
    return {
        "name": ref.name,
        "kind": ref.__struct_config__.tag,
        "sha256": hashlib.sha256(file.read_bytes()).hexdigest(),
        "use_audio": getattr(ref, "use_audio", False),
    }


def take(payload, source: File, out: Outputs) -> Take:
    """`source`, a new file (an mp4 `free` box added), and as `given` what it got."""
    given = {
        "prompt": payload.prompt,
        "duration_s": payload.duration_s,
        "continues": getattr(payload, "continue_from", None) is not None,
        "references": [reference(ref) for ref in payload.references],
    }
    video = source.read_bytes() + (24).to_bytes(4) + b"free" + os.urandom(16)
    warnings = [f"rendered {PLATE_S} s of {payload.duration_s:g} s"] if payload.duration_s > PLATE_S else []
    return Take(
        out.save_bytes(video, media_type="video/mp4"),
        out.save_bytes(json.dumps(given).encode(), media_type="application/json"),
        warnings,
    )


@app.entrypoint()
def shoot(payload: ShotRequest, out: Outputs) -> Take:
    return take(payload, payload.plate.video, out)


@app.entrypoint()
def pieces(payload: Pieces, out: Outputs) -> Take:
    return take(payload, payload.plate.video, out)


@app.entrypoint()
def bare(payload: Bare, out: Outputs) -> Take:
    return take(payload, payload.references[0].video, out)
