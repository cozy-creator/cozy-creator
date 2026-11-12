# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]
# ///
"""The cli suite's on-demand holder (cl-905): a call that builds BiRefNet on demand, holding its executor while another
process sharing the machine's store reads that model, as the call's own stage would."""

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


class Held(msgspec.Struct):
    seconds: float


@app.entrypoint(accelerator=False, defaults={"extra": "zhengpeng7/birefnet@0.1.0"}, on_demand=("extra",))
def hold(payload: Hold, extra: Extra, tel: Telemetry) -> Held:
    """Says it holds, then sleeps; never reads `extra` itself."""
    with tel.stage("holding"):
        time.sleep(payload.seconds)
    return Held(seconds=payload.seconds)
