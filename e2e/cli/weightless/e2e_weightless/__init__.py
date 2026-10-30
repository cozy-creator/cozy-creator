"""The cli suite's weightless requests: from cozy through TensorD, the Runtime and TensorFS to the executor's
reply and back, with no model, so they run on any computer."""

import os
import time

import msgspec

from cozy_runtime.author import App, File, Files, Telemetry

app = App()


class Text(msgspec.Struct, forbid_unknown_fields=True):
    text: str


class Reversed(msgspec.Struct):
    text: str


@app.entrypoint(accelerator=False)
def reverse(payload: Text) -> Reversed:
    return Reversed(text=payload.text[::-1])


class Wait(msgspec.Struct, forbid_unknown_fields=True):
    seconds: float


class Waited(msgspec.Struct):
    seconds: float


@app.entrypoint(accelerator=False)
def wait(payload: Wait) -> Waited:
    """A run that stays in progress: followed, snapshotted and canceled by the suite."""
    time.sleep(payload.seconds)
    return Waited(seconds=payload.seconds)


@app.entrypoint(accelerator=False)
def silent(payload: Wait, tel: Telemetry) -> Waited:
    """Says it waits, then sleeps without a word: nothing judges a run's progress (cl-663)."""
    tel.progress(0.0, stage="waiting silently")
    time.sleep(payload.seconds)
    return Waited(seconds=payload.seconds)


@app.entrypoint(accelerator=False)
def orphan(payload: Wait, tel: Telemetry) -> Waited:
    """Forks a child that holds the executor's channel open, names both, then waits: the suite
    kills this executor, and its run must end at once."""
    child = os.fork()
    if child == 0:
        try:
            time.sleep(payload.seconds)
        finally:
            os._exit(0)
    tel.progress(0.0, stage=f"executor {os.getpid()} forked {child}")
    time.sleep(payload.seconds)
    return Waited(seconds=payload.seconds)


class Nothing(msgspec.Struct, forbid_unknown_fields=True):
    pass


class Seen(msgspec.Struct):
    assets: list[str]


@app.entrypoint(accelerator=False)
def fidelities(payload: Nothing, assets: Files[File]) -> Seen:
    """Each attached asset as the executor was granted it: `label=fidelity`."""
    return Seen(assets=[f"{assets.info(i).label}={assets.info(i).fidelity}" for i in range(len(assets))])
