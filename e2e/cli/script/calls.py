# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[adapters]>=0.45", "msgspec>=0.19"]
# ///
"""The cli suite's orchestrators (cl-1077): `edit` loads no model and calls `used` (e2e/calls-used), never `unused`
(e2e/calls-unused), as H3's `edit` calls its segments and never `upscale`'s regenerate. `adapt` calls `adapted` with a LoRA
(e2e/calls-lora) on its `base` (e2e/calls-base) beside its own `extra` (e2e/calls-other), as H3's `edit` puts its edit LoRA on a
segment's base beside the turbo LoRA. `early` reads e2e/calls-early itself; `late` calls `late_call`, which reads
e2e/calls-late, as LTX and H3's `edit` met on one machine. `needs` loads e2e/calls-needed and has e2e/calls-extra on demand,
never read (cl-1102); `cold` the same with e2e/calls-cold, which loads slowly, and e2e/calls-waits (cl-1118)."""

import asyncio
import time

import msgspec
import torch

from cozy_runtime.author import App, Loader, Lora, Model, ModelChoice, Serving, Telemetry, call, uses_components

app = App()


class Extra(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())


class Ask(msgspec.Struct, kw_only=True):
    seconds: float = 0


class Said(msgspec.Struct):
    used: str


@app.entrypoint(private=True, accelerator=False, defaults={"extra": "e2e/calls-used@0.1.0"})
def used(payload: Ask, extra: Extra) -> Said:
    return Said(used="e2e/calls-used")


@app.entrypoint(private=True, accelerator=False, defaults={"extra": "e2e/calls-unused@0.1.0"})
def unused(payload: Ask, extra: Extra) -> Said:
    return Said(used="e2e/calls-unused")


@app.entrypoint(accelerator=False)
async def edit(payload: Ask) -> Said:
    """Holds `seconds` while its prefetch runs, then calls `used`."""
    await asyncio.sleep(payload.seconds)
    return await call(used, Ask())


class Proj(torch.nn.Module):
    def __init__(self) -> None:
        super().__init__()
        self.proj = torch.nn.Linear(8192, 8192, bias=False)


class Graph:
    def __init__(self) -> None:
        self.components = {"transformer": Proj()}


class Transformer(Model[Graph]):
    """e2e/calls-base's transformer, one linear `proj` (256 MiB) that e2e/calls-lora adapts, as cli/script/lora.py's."""

    serving = Serving(encoded_leaves=False, loras=("transformer",))
    graph: Graph

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: Graph())

    @uses_components("transformer")
    def read(self) -> float:
        with torch.no_grad():
            return float(self.graph.components["transformer"].proj(torch.ones(1, 8192)).double().sum())


@app.entrypoint(
    private=True,
    accelerator=False,
    defaults={"base": "e2e/calls-base@0.1.0", "extra": "e2e/calls-other@0.1.0"},
    components={"base": ("transformer",)},
)
def adapted(payload: Ask, base: Transformer, extra: Extra) -> Said:
    return Said(used=f"e2e/calls-base under e2e/calls-lora ({base.read():.3f}), and e2e/calls-other")


@app.entrypoint(accelerator=False)
async def adapt(payload: Ask) -> Said:
    """Calls `adapted` with e2e/calls-lora on its base."""
    return await call(adapted, Ask(), models={"base": ModelChoice(lora=(Lora("e2e/calls-lora", 1.0),))})


@app.entrypoint(accelerator=False, defaults={"extra": "e2e/calls-early@0.1.0"})
def early(payload: Ask, extra: Extra) -> Said:
    return Said(used="e2e/calls-early")


@app.entrypoint(private=True, accelerator=False, defaults={"extra": "e2e/calls-late@0.1.0"})
def late_call(payload: Ask, extra: Extra) -> Said:
    return Said(used="e2e/calls-late")


@app.entrypoint(accelerator=False)
async def late(payload: Ask) -> Said:
    """Calls `late_call` at once: its root holds no GPU."""
    return await call(late_call, Ask())


class Later(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())


@app.entrypoint(
    accelerator=False,
    defaults={"extra": "e2e/calls-needed@0.1.0", "later": "e2e/calls-extra@0.1.0"},
    on_demand=("later",),
)
def needs(payload: Ask, extra: Extra, later: Later, tel: Telemetry) -> Said:
    """Holds `seconds` with `extra` loaded and `later`, on demand, never read."""
    with tel.stage("holding"):
        time.sleep(payload.seconds)
    return Said(used="e2e/calls-needed")


class Slow(Model[object]):
    """A model whose load takes 15 s, as a cold H3 call's executor start and load take minutes (cl-1118)."""

    graph: object

    def load(self, loader: Loader) -> None:
        time.sleep(15)
        self.graph = loader.construct(lambda config: object())


@app.entrypoint(
    accelerator=False,
    defaults={"slow": "e2e/calls-cold@0.1.0", "later": "e2e/calls-waits@0.1.0"},
    on_demand=("later",),
)
def cold(payload: Ask, slow: Slow, later: Later, tel: Telemetry) -> Said:
    """Holds `seconds` with `slow` loaded and `later`, on demand, never read (cl-1118)."""
    with tel.stage("holding"):
        time.sleep(payload.seconds)
    return Said(used="e2e/calls-cold")
