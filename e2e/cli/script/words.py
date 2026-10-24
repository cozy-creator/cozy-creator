# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.30", "msgspec>=0.19"]
# ///
"""The cli suite's script: an ordinary one-file package (xs-099). Its function takes the machine's GPU when it
has one (`accelerator=True`), so on a computer with none it runs on the CPU."""

import msgspec

from cozy_runtime.author import App

app = App()


class Words(msgspec.Struct, forbid_unknown_fields=True):
    text: str


class Counted(msgspec.Struct):
    words: int


@app.entrypoint(accelerator=True)
def count(payload: Words) -> Counted:
    return Counted(words=len(payload.text.split()))
