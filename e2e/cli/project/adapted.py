# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.37", "msgspec>=0.19"]
# ///
"""A shot renderer with a project adapter: weightless, its prompt saying what the adapter read."""

import json

import msgspec
from cozy_runtime.author import App, File, Invocation, Outputs, ProjectShot, VideoFile

app = App()


class Plate(msgspec.Struct, kw_only=True):
    video: VideoFile


class Request(msgspec.Struct, kw_only=True):
    prompt: str
    plate: Plate


class Take(msgspec.Struct):
    video: File
    given: File
    warnings: list[str]


@app.entrypoint()
def shoot(payload: Request, out: Outputs) -> Take:
    """Its plate, and as `given` the prompt it got."""
    given = out.save_bytes(json.dumps({"prompt": payload.prompt}).encode(), media_type="application/json")
    return Take(out.save_bytes(payload.plate.video.read_bytes(), media_type="video/mp4"), given, [])


@app.project_adapter("shot")
def adapt(payload: ProjectShot) -> Invocation:
    """Its own vocabulary, nothing the project format names."""
    said = [f"adapted v{payload.version} {payload.name}: {payload.summary}", *(f"{beat.at:g}: {beat.description}" for beat in payload.beats)]
    said += [payload.soundscape, payload.music]
    return Invocation(function="shoot", input={"prompt": " | ".join(said), "plate": payload.plate})
