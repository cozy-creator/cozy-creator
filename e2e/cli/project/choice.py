# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.45.0", "msgspec>=0.19"]
# ///
"""The choice cell's fixture: sizes and counts as choices, as H3 Upscale's `to` and qwen-image-2's `megapixels`. It says in
`given` what its function got."""

from typing import Literal

import msgspec
from cozy_runtime.author import App

app = App()


class Box(msgspec.Struct, kw_only=True):
    to: Literal["1080p", "1440p", "2160p"] = "1440p"
    megapixels: Literal[2, 4] = 4
    mode: Literal["fast", "full"] = "full"


class Given(msgspec.Struct):
    given: str


@app.entrypoint(accelerator=False)
def size(payload: Box) -> Given:
    """What it was given."""
    return Given(f"to={payload.to} megapixels={payload.megapixels} mode={payload.mode}")
