"""xs-132 proof fixture: a CPU orchestrator whose one step calls another package's function (birefnet)."""

from typing import Annotated

import msgspec
from birefnet import remove_background

from cozy_runtime.author import App, AssetBound, Context, ImageAsset

app = App()


class CutInput(msgspec.Struct):
    image: Annotated[
        ImageAsset,
        AssetBound(max_bytes=64 << 20, max_decoded_bytes=256 << 20, media_types=("image/png", "image/jpeg", "image/webp")),
    ]


class CutOutput(msgspec.Struct):
    cut: bool


async def cut(ctx: Context, payload: CutInput) -> CutOutput:
    await remove_background(image=payload.image)
    return CutOutput(cut=True)


app.entrypoint(cut, accelerator=False)
