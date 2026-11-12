# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]
# ///
"""The cli suite's on-demand holder (cl-966): a call with BiRefNet as an on-demand slot, which holds its executor for a
while and reads that model only when asked, as a stage would."""

import time

import msgspec

from cozy_runtime.author import App, Loader, Model, Telemetry, uses_components

app = App()


class Extra(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())

    @uses_components("model")
    def forward(self) -> None:
        return None


class Hold(msgspec.Struct, kw_only=True):
    seconds: float = 600
    read: bool = False


class Held(msgspec.Struct):
    seconds: float


@app.entrypoint(accelerator=False, defaults={"extra": "zhengpeng7/birefnet@0.1.0"}, on_demand=("extra",))
def hold(payload: Hold, extra: Extra, tel: Telemetry) -> Held:
    """Says it holds and sleeps, then reads `extra` when asked to."""
    with tel.stage("holding"):
        time.sleep(payload.seconds)
    if payload.read:
        with tel.stage("reading it"):
            extra.forward()
    return Held(seconds=payload.seconds)
