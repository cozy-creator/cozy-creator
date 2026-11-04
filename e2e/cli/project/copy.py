# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.38", "msgspec>=0.19"]
# ///
"""The project cell's package with no seed: weightless, a new picture from the one it is given, so one result per inputs."""

import msgspec
from cozy_runtime.author import App, File, Invocation, Operation, Outputs

app = App()


class Picture(msgspec.Struct, kw_only=True):
    image: File


class Copied(msgspec.Struct):
    image: File


@app.entrypoint()
def copy(payload: Picture, out: Outputs) -> Copied:
    return Copied(out.save_bytes(payload.image.read_bytes() + b"copied", media_type="image/png"))


@app.project_adapter("copy")
def adapt(op: Operation) -> list[Invocation]:
    picture = next(read for read in op.reads if "image" in read.files)
    if picture.files["image"].alpha >= 0.05:
        return []
    return [Invocation(function="copy", input={"image": picture.files["image"].sha256})]
