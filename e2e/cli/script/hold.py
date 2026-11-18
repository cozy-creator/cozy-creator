# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.45", "msgspec>=0.19"]
# ///
"""The cli suite's model in use (cli/disk.sh `disk_in_use`, Paul 11-17): `hold` reads every weight of e2e/disk-1 (e2e/isolated.sh's
`disk`: six F16 tensors w.0..w.5 of 64 MiB, component `model`), waits `seconds` mid-call while the cell asks the disk for another
model's room, then reads them again and fails unless their sum is the same: its model never left the disk under it."""

import time

import msgspec
import torch

from cozy_runtime.author import App, Context, Loader, Model, Telemetry, uses_components

app = App()


class Tensors(torch.nn.Module):
    def __init__(self) -> None:
        super().__init__()
        self.w = torch.nn.ParameterList(
            torch.nn.Parameter(torch.empty(32 << 20, dtype=torch.float16), requires_grad=False) for _ in range(6)
        )


class Graph:
    def __init__(self) -> None:
        self.components = {"model": Tensors()}


class Disk(Model[Graph]):
    graph: Graph

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: Graph())

    @uses_components("model")
    def total(self) -> float:
        return sum(float(w.double().sum()) for w in self.graph.components["model"].w)


class Hold(msgspec.Struct, kw_only=True):
    seconds: int = 120


class Held(msgspec.Struct):
    sum: float


@app.entrypoint(accelerator=False, defaults={"weights": "e2e/disk-1@0.1.0"})
def hold(ctx: Context, payload: Hold, weights: Disk, tel: Telemetry) -> Held:
    """Reads its model, holds it `seconds`, and reads it again."""
    with tel.stage("read"):
        before = weights.total()
    with tel.stage("holding"):
        for second in range(payload.seconds):
            ctx.raise_if_cancelled()
            ctx.progress(second, payload.seconds)
            time.sleep(1)
    with tel.stage("read again"):
        after = weights.total()
    if after != before:
        raise RuntimeError(f"e2e/disk-1 read {before}, then {after}")
    return Held(after)
