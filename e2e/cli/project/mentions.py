# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""A prompt as the function gets it: its text, and each reference its mentions sent, by key and picture width."""

import msgspec
from cozy_runtime.author import App
from cozy_runtime.fields import Image, Prompt

app = App()


class Asked(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()


class Said(msgspec.Struct):
    text: str
    keys: list[str]
    widths: list[int]


@app.entrypoint(accelerator=False)
def mentions(payload: Asked) -> Said:
    """What the prompt's mentions sent."""
    refs = payload.instructions.references
    return Said(payload.instructions.text, list(refs.keys()), [ref.file.probe().width for ref in refs.values()])
