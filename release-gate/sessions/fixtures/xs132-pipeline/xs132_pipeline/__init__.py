"""xs-132 proof fixture: a CPU orchestrator whose one step calls another package's function (birefnet)."""

from typing import Annotated

import msgspec
from birefnet import RemoveBackgroundInput, remove_background

from cozy_runtime.author import App, Context, call
from cozy_runtime.fields import FileBound
from cozy_runtime.media import ImageFile

app = App()


class CutInput(msgspec.Struct):
    image: Annotated[
        ImageFile,
        FileBound(max_bytes=64 << 20, max_decoded_bytes=256 << 20, media_types=("image/png", "image/jpeg", "image/webp")),
    ]


class CutOutput(msgspec.Struct):
    cut: bool


async def cut(ctx: Context, payload: CutInput) -> CutOutput:
    await call(remove_background, payload=RemoveBackgroundInput(image=payload.image))
    return CutOutput(cut=True)


app.entrypoint(cut, accelerator=False)
