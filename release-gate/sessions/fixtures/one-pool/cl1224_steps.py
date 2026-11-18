# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.39", "msgspec>=0.19"]
# ///
"""one-pool.sh's run (cl-1224): steps on a CPU, so another computer's listing names it."""
import time

import msgspec

from cozy_runtime.author import App, Context, Telemetry

app = App()


class Steps(msgspec.Struct):
    steps: int = 60
    seconds: float = 2.0


class Stepped(msgspec.Struct):
    steps: int


@app.entrypoint(accelerator=False)
def cl1224_steps(ctx: Context, payload: Steps, tel: Telemetry) -> Stepped:
    with tel.stage("stepping"):
        for i in range(payload.steps):
            ctx.progress(i, payload.steps)
            time.sleep(payload.seconds)
    return Stepped(payload.steps)
