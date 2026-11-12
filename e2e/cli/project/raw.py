# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]==0.45.0", "tensorfs==0.24.1", "msgspec>=0.19"]
# ///
"""web/ignored.js's functions, which say what they got: `pictures` a prompt of pictures, `words` words only (cl-944).

Pinned to the release candidate's own series, so the candidate Runtime stands in for its lock (tensord's environment.rs
`stands_in`): a lock on a newer published patch runs that patch instead. Move the pins with the candidate's series."""

import msgspec
from cozy_runtime.author import App
from cozy_runtime.fields import Image, Prompt

app = App()


class Pictures(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()


class Words(msgspec.Struct, kw_only=True):
    prompt: str = ""


class Got(msgspec.Struct):
    text: str
    kept: list[str]


@app.entrypoint()
def pictures(payload: Pictures) -> Got:
    """Its prompt's text and the references it kept."""
    return Got(payload.instructions.text, list(payload.instructions.references))


@app.entrypoint()
def words(payload: Words) -> Got:
    """Its words."""
    return Got(payload.prompt, [])
