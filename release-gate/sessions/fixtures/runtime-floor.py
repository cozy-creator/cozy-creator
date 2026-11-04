# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.39.4", "msgspec>=0.19"]
# ///
"""A floor above the pair of the session's machine (`runtime-floor`): its environment takes its own Runtime, and says which."""
import importlib.metadata

import msgspec

from cozy_runtime.author import App

app = App()


class In(msgspec.Struct):
    text: str = "x"


class Out(msgspec.Struct):
    runtime: str
    tensorfs: str


@app.entrypoint(accelerator=False)
def versions(payload: In) -> Out:
    held = importlib.metadata.version
    return Out(held("cozy-runtime"), held("tensorfs"))
