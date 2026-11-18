# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[adapters]>=0.45", "msgspec>=0.19"]
# ///
"""The cli suite's handed-LoRA script (cl-1281): Orbit's, MOSS's and H3 edit's shape. `handed` loads no model; its LoRA slot, a
Weights with a default and a `needs` rule, goes to the call that applies it by its ref. `encoded` is lora_slots.py's. Its own script:
a Runtime before cozy-runtime #2295 refuses the rule when it reads the file, which would stop lora_slots.py's cells too."""

from typing import cast

import msgspec
import torch
from cozy_runtime.author import (
    App,
    Loader,
    Lora,
    Model,
    ModelChoice,
    Serving,
    Weights,
    call,
    uses_components,
)

app = App()
BASE = "e2e/lora-base@0.1.0"
PROJ = "e2e/lora-proj@0.1.0"
DIT = ("transformer",)


class Proj(torch.nn.Module):
    def __init__(self) -> None:
        super().__init__()
        self.proj = torch.nn.Linear(256, 256, bias=False)


class Parts:
    def __init__(self, components: dict[str, torch.nn.Module]) -> None:
        self.components = components


class Transformer(Model[Parts]):
    serving = Serving(encoded_leaves=False, loras=DIT)
    graph: Parts

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: Parts({"transformer": Proj()}))

    @uses_components("transformer")
    def read(self) -> float:
        with torch.no_grad():
            transformer = cast(Proj, self.graph.components["transformer"])
            return float(transformer.proj(torch.ones(1, 256)).double().sum())


class Ask(msgspec.Struct, kw_only=True):
    label: str = ""


class Hand(msgspec.Struct, kw_only=True):
    #: `handed` hands its LoRA slot to its call, which only this request fetches (its `needs` rule).
    styled: bool = True


class Read(msgspec.Struct):
    sum: float


@app.entrypoint(accelerator=False, defaults={"model": BASE}, components={"model": DIT})
def encoded(payload: Ask, model: Transformer) -> Read:
    """What `model`'s transformer makes of ones."""
    return Read(model.read())


@app.entrypoint(accelerator=False, defaults={"lora": PROJ}, needs={"lora": "request.styled"})
async def handed(payload: Hand, lora: Weights) -> Read:
    """`encoded` called with this slot's LoRA on its model when `styled`, else without: the slot's default, or the
    request's `model.lora=` in its place."""
    models = {"model": ModelChoice(lora=(Lora(lora.ref),))} if payload.styled else None
    return await call(encoded, Ask(), models=models)
