# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]
# ///
"""tfs-300's link cells (e2e/cli/link.sh): calls whose weights or reads are the measurement. Each times its own read in the
call and says it, so a cell needs no clock of its own around it."""

import time

import msgspec

from cozy_runtime.author import App, Loader, Model, Telemetry

app = App()


class Weights(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())


class Ask(msgspec.Struct, kw_only=True):
    seconds: float = 0
    read: bool = True


class Took(msgspec.Struct):
    read_s: float
    began_unix_ms: int = 0


@app.entrypoint(accelerator=False, defaults={"weights": "e2e/link-a@0.1.0"})
def read(payload: Ask, weights: Weights) -> Took:
    """Its weights come before it starts; it builds them."""
    began = time.monotonic()
    _ = weights.graph
    return Took(read_s=time.monotonic() - began)


@app.entrypoint(accelerator=False, defaults={"extra": "e2e/link-a@0.1.0"}, on_demand=("extra",))
def hold(payload: Ask, extra: Weights, tel: Telemetry) -> Took:
    """Holds, then reads its on-demand model: a started call's read of weights not here (model_need)."""
    with tel.stage("holding"):
        time.sleep(payload.seconds)
    began, at = time.monotonic(), int(time.time() * 1000)
    if payload.read:
        with tel.stage("reading it"):
            _ = extra.graph
    return Took(read_s=time.monotonic() - began, began_unix_ms=at)
