"""estimate_plan's broken estimate (cl-1286, e2e/cli/estimate.sh): a size variable naming a field its request lacks."""

import msgspec

from cozy_runtime.author import App

app = App()


class Shot(msgspec.Struct, kw_only=True):
    duration: float = 5.0


class Made(msgspec.Struct):
    seconds: float


@app.entrypoint(accelerator=False, estimate={"size": {"seconds": "request.durtion"}})
def broken(payload: Shot) -> Made:
    return Made(seconds=payload.duration)
