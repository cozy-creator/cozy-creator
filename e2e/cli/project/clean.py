# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.38", "msgspec>=0.19"]
# ///
"""The project cell's clean package: weightless, a new picture from the one it is given, and no seed, so one result per inputs."""

import msgspec
from cozy_runtime.author import App, File, Invocation, Operation, Outputs

app = App()


class Picture(msgspec.Struct, kw_only=True):
    image: File


class Cleaned(msgspec.Struct):
    image: File


@app.entrypoint()
def clean(payload: Picture, out: Outputs) -> Cleaned:
    return Cleaned(out.save_bytes(payload.image.read_bytes() + b"cleaned", media_type="image/png"))


@app.project_adapter("clean")
def adapt(op: Operation) -> list[Invocation]:
    picture = next(read for read in op.reads if "image" in read.files)
    if picture.files["image"].alpha >= 0.05:
        return []
    return [Invocation(function="clean", input={"image": picture.files["image"].sha256})]
