# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[adapters]>=0.45", "msgspec>=0.19"]
# ///
"""The cli suite's LoRA-slot script (cl-1241): two slots of e2e/lora-base, as FLUX.3 Klein's box has them. The first,
`text_encoder`, reads its text encoder with a class that takes no LoRA; the second, `model`, reads its transformer (one
linear, `proj`, which e2e/lora-proj adapts) with a class that does. It says what `proj` makes of ones."""

import msgspec
import torch
from cozy_runtime.author import App, Loader, Model, Serving, uses_components

app = App()
BASE = "e2e/lora-base@0.1.0"


class Proj(torch.nn.Module):
    def __init__(self) -> None:
        super().__init__()
        self.proj = torch.nn.Linear(256, 256, bias=False)


class Vector(torch.nn.Module):
    def __init__(self) -> None:
        super().__init__()
        self.w = torch.nn.Parameter(torch.empty(4194304), requires_grad=False)


class Parts:
    def __init__(self, components: dict[str, torch.nn.Module]) -> None:
        self.components = components


class Encoder(Model[Parts]):
    serving = Serving(encoded_leaves=False)
    graph: Parts

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: Parts({"text_encoder": Vector()}))

    @uses_components("text_encoder")
    def read(self) -> float:
        return float(self.graph.components["text_encoder"].w[:8].double().sum())


class Transformer(Model[Parts]):
    serving = Serving(encoded_leaves=False, loras=("transformer",))
    graph: Parts

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: Parts({"transformer": Proj()}))

    @uses_components("transformer")
    def read(self) -> float:
        with torch.no_grad():
            return float(self.graph.components["transformer"].proj(torch.ones(1, 256)).double().sum())


class Ask(msgspec.Struct, kw_only=True):
    label: str = ""


class Read(msgspec.Struct):
    sum: float
    encoded: float


@app.entrypoint(accelerator=False, defaults={"text_encoder": BASE, "model": BASE},
                components={"text_encoder": ("text_encoder",), "model": ("transformer",)})  # fmt: skip
def encoded(payload: Ask, text_encoder: Encoder, model: Transformer) -> Read:
    """What `model`'s transformer makes of ones, beside its text encoder's first values."""
    return Read(model.read(), text_encoder.read())
