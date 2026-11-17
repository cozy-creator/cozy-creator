# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""A picture at a box-shaped input, as `cozy run` attaches it (cl-1183): the call reads it at `picture.file`."""

import hashlib

import msgspec
from cozy_runtime.author import App
from cozy_runtime.fields import Image

app = App()


class Given(msgspec.Struct, kw_only=True):
    picture: Image


class Read(msgspec.Struct):
    sha256: str
    width: int


@app.entrypoint(accelerator=False)
def read(payload: Given) -> Read:
    """The picture's bytes and width, as the function read them."""
    return Read(hashlib.sha256(payload.picture.file.read_bytes()).hexdigest(), payload.picture.file.probe().width)
