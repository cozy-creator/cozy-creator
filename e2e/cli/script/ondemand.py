# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]
# ///
"""The cli suite's on-demand holder (cl-966): a call with an on-demand slot (BiRefNet; `keep`'s, fontbox), which holds its
executor for a while and reads that model only when asked, as a stage would."""

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


@app.entrypoint(accelerator=False, defaults={"extra": "zhengpeng7/birefnet@0.1.0", "base": "lilylilith/fontbox@0.1.0"},
               on_demand=("extra",))
def hold(payload: Hold, extra: Extra, base: Extra, tel: Telemetry) -> Held:
    """Says it holds and sleeps, then reads `extra` when asked to: its first read builds it (beside `base`, it's built
    on first use, not with the call's load)."""
    with tel.stage("holding"):
        time.sleep(payload.seconds)
    if payload.read:
        with tel.stage("reading it"):
            _ = extra.graph
    return Held(seconds=payload.seconds)


@app.entrypoint(accelerator=False, defaults={"extra": "lilylilith/fontbox@0.1.0"}, on_demand=("extra",))
def keep(payload: Hold, extra: Extra, tel: Telemetry) -> Held:
    """Holds with fontbox as its on-demand model, never reading it."""
    with tel.stage("holding"):
        time.sleep(payload.seconds)
    return Held(seconds=payload.seconds)
