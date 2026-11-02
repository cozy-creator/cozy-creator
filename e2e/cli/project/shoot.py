# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.37", "msgspec>=0.19"]
# ///
"""The project cell's shot renderer: weightless, and it says what it was given."""

import hashlib
import json
import os

import msgspec
from cozy_runtime.author import App, File, Outputs, ShotRequest

app = App()


class Take(msgspec.Struct):
    video: File
    given: File


@app.entrypoint()
def shoot(payload: ShotRequest, out: Outputs) -> Take:
    """Its plate, a new file (an mp4 `free` box added), and as `given` the references it got."""
    given = []
    for ref in payload.references:
        file = next(getattr(ref, field) for field in ("video", "audio", "image") if hasattr(ref, field))
        given.append({
            "name": ref.name,
            "kind": ref.__struct_config__.tag,
            "sha256": hashlib.sha256(file.read_bytes()).hexdigest(),
            "use_audio": getattr(ref, "use_audio", False),
        })
    video = payload.plate.video.read_bytes() + (24).to_bytes(4) + b"free" + os.urandom(16)
    return Take(
        out.save_bytes(video, media_type="video/mp4"),
        out.save_bytes(json.dumps(given).encode(), media_type="application/json"),
    )
