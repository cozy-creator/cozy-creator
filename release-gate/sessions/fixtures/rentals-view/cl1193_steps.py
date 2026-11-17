# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.39", "msgspec>=0.19"]
# ///
"""rentals-view.sh's worker run (cl-1193): steps on its CPU for a while, saying how far it is, so another computer's listing shows it."""
import time

import msgspec

from cozy_runtime.author import App, Context, Telemetry

app = App()


class Steps(msgspec.Struct):
    steps: int = 180
    seconds: float = 2.0


class Stepped(msgspec.Struct):
    steps: int


@app.entrypoint(accelerator=False)
def cl1193_steps(ctx: Context, payload: Steps, tel: Telemetry) -> Stepped:
    with tel.stage("stepping"):
        for i in range(payload.steps):
            time.sleep(payload.seconds)
            ctx.progress(i + 1, payload.steps)
    return Stepped(payload.steps)
