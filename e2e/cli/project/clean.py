# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.30", "msgspec>=0.19"]
# ///
"""The project cell's cleaner: weightless, a new picture from the one it is given."""

import msgspec
from cozy_runtime.author import App, File, Outputs

app = App()


class Picture(msgspec.Struct, kw_only=True):
    image: File


class Cleaned(msgspec.Struct):
    image: File


@app.entrypoint()
def clean(payload: Picture, out: Outputs) -> Cleaned:
    return Cleaned(out.save_bytes(payload.image.read_bytes() + b"cleaned", media_type="image/png"))
