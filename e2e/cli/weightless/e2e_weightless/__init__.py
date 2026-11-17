"""The cli suite's weightless requests: from cozy through TensorD, the Runtime and TensorFS to the executor's
reply and back, with no model, so they run on any computer."""

import time

import msgspec

from cozy_runtime.author import App, Context, Telemetry
from cozy_runtime.media import File, Files

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


class Steps(msgspec.Struct, forbid_unknown_fields=True):
    steps: int
    seconds: float


@app.entrypoint(accelerator=False)
def steps(ctx: Context, payload: Steps) -> Waited:
    """`steps` steps of `seconds` each: canceled mid-steps, it stops at the next and keeps what it measured (cl-1154)."""
    for step in range(payload.steps):
        time.sleep(payload.seconds)
        ctx.progress(step, payload.steps)
    return Waited(seconds=payload.steps * payload.seconds)


@app.entrypoint(accelerator=False)
def silent(payload: Wait, tel: Telemetry) -> Waited:
    """Says it waits, then sleeps without a word: nothing judges a run's progress (cl-663)."""
    with tel.stage("waiting silently"):
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
