# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.38", "msgspec>=0.19"]
# ///
"""The project cell's package with no seed: weightless, a new picture from the one it is given, so one result per inputs."""

import msgspec
from cozy_runtime.author import App, Outputs
from cozy_runtime.media import File

app = App()


class Picture(msgspec.Struct, kw_only=True):
    image: File


class Copied(msgspec.Struct):
    image: File


@app.entrypoint()
def copy(payload: Picture, out: Outputs) -> Copied:
    return Copied(out.save_bytes(payload.image.read_bytes() + b"copied", media_type="image/png"))
