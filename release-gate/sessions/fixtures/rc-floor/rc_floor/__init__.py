"""rental-boot-first's code (cl-1069): a Runtime floor only the open RC's Runtime meets; it says which Runtime ran it."""
import importlib.metadata

import msgspec

from cozy_runtime.author import App

app = App()


class In(msgspec.Struct):
    text: str = "x"


class Ran(msgspec.Struct):
    runtime: str


@app.entrypoint(accelerator=False)
def runtime(payload: In) -> Ran:
    return Ran(importlib.metadata.version("cozy-runtime"))
