"""rental-boot-first's code (cl-1069): a Runtime floor the session sets; it says which Runtime ran it."""
import importlib.metadata
import time

import msgspec

from cozy_runtime.author import App

app = App()


class In(msgspec.Struct):
    text: str = "x"


class Wait(msgspec.Struct):
    seconds: float = 120.0


class Ran(msgspec.Struct):
    runtime: str


@app.entrypoint(accelerator=False)
def runtime(payload: In) -> Ran:
    return Ran(importlib.metadata.version("cozy-runtime"))


@app.entrypoint(accelerator=False)
def wait(payload: Wait) -> Ran:
    time.sleep(payload.seconds)
    return Ran(importlib.metadata.version("cozy-runtime"))
