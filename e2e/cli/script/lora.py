# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[adapters]>=0.45", "msgspec>=0.19"]
# ///
"""The cli suite's LoRA script (tfs-300): `model` reads only the `transformer` of e2e/lora-base, a model of a transformer and a
text encoder, as qwen-image-2's `model` reads only Noct Q's transformer. The transformer is one linear, `proj`, which
e2e/lora-proj adapts; it says what `proj` makes of ones. Its weights are e2e/isolated.sh's (`lorabase`)."""

import msgspec
import torch
from cozy_runtime.author import App, Loader, Model, Serving, uses_components

app = App()


class Proj(torch.nn.Module):
    def __init__(self) -> None:
        super().__init__()
        self.proj = torch.nn.Linear(256, 256, bias=False)


class Graph:
    def __init__(self) -> None:
        self.components = {"transformer": Proj()}


class Transformer(Model[Graph]):
    serving = Serving(encoded_leaves=False, loras=("transformer",))
    graph: Graph

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: Graph())

    @uses_components("transformer")
    def read(self) -> float:
        with torch.no_grad():
            return float(self.graph.components["transformer"].proj(torch.ones(1, 256)).double().sum())


class Ask(msgspec.Struct, kw_only=True):
    label: str = ""


class Read(msgspec.Struct):
    sum: float


@app.entrypoint(accelerator=False, defaults={"model": "e2e/lora-base@0.1.0"}, components={"model": ("transformer",)})
def adapted(payload: Ask, model: Transformer) -> Read:
    """What `model`'s transformer makes of ones."""
    return Read(model.read())
