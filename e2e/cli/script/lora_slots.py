# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[adapters]>=0.45", "msgspec>=0.19"]
# ///
"""The cli suite's LoRA-slot script (cl-1241): where a LoRA naming no slot goes, by the slots whose class takes LoRAs
(`Serving(loras=...)`). `Encoder` reads e2e/lora-base's text encoder and takes none; `Transformer` reads a transformer (one
linear, `proj`, which e2e/lora-proj adapts) of e2e/lora-base or e2e/lora-base-b (another `proj`) and takes them. Each
function is the shape of first-party ones: `encoded` FLUX.3 Klein's `generate`, `chosen` Klein's, Fibo's and Krea 2's
`image` box, `edited` qwen-image's functions, `pair` two models at once, `plain` none that takes one. `handed` (cl-1281) is
Orbit's, MOSS's and H3 edit's shape: it loads no model, and its LoRA slot, with a default and a `needs` rule, goes to the
call that applies it by its ref."""

from typing import cast

import msgspec
import torch
from cozy_runtime.author import App, Loader, Lora, Model, ModelChoice, Serving, Weights, call, uses_components

app = App()
BASE = "e2e/lora-base@0.1.0"
OTHER = "e2e/lora-base-b@0.1.0"
PROJ = "e2e/lora-proj@0.1.0"
TEXT = ("text_encoder",)
DIT = ("transformer",)


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
        encoder = cast(Vector, self.graph.components["text_encoder"])
        return float(encoder.w[:8].double().sum())


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


class Choose(msgspec.Struct, kw_only=True):
    #: `model` reads this request, else `full` (the `needs` rules: Klein's box's `turbo`).
    turbo: bool = True


class Edit(msgspec.Struct, kw_only=True):
    #: `edit_model` reads this request, else `model` (qwen-image's edit model, for a request with pictures).
    edit: bool = False


class Hand(msgspec.Struct, kw_only=True):
    #: `handed` hands its LoRA slot to its call, which only this request fetches (its `needs` rule).
    styled: bool = True


class Read(msgspec.Struct):
    sum: float
    encoded: float = 0.0


@app.entrypoint(accelerator=False, defaults={"text_encoder": BASE, "model": BASE},
                components={"text_encoder": TEXT, "model": DIT})  # fmt: skip
def encoded(payload: Ask, text_encoder: Encoder, model: Transformer) -> Read:
    """What `model`'s transformer makes of ones, beside its text encoder's first values: the text encoder first, as Klein's."""
    return Read(model.read(), text_encoder.read())


@app.entrypoint(accelerator=False, defaults={"text_encoder": BASE, "full": OTHER, "model": BASE},
                components={"text_encoder": TEXT, "full": DIT, "model": DIT},
                needs={"full": "!request.turbo", "model": "request.turbo"})  # fmt: skip
def chosen(payload: Choose, text_encoder: Encoder, full: Transformer, model: Transformer) -> Read:
    """`model`'s transformer, else `full`'s, the one the request needs: two models, alternatives."""
    return Read((model if payload.turbo else full).read(), text_encoder.read())


@app.entrypoint(accelerator=False, defaults={"edit_model": OTHER, "model": BASE, "text_encoder": BASE},
                components={"edit_model": DIT, "model": DIT, "text_encoder": TEXT},
                needs={"edit_model": "request.edit"})  # fmt: skip
def edited(payload: Edit, edit_model: Transformer, model: Transformer, text_encoder: Encoder) -> Read:
    """`model`'s transformer, which every request loads, or with `edit` the edit model's, which it loads on demand."""
    return Read((edit_model if payload.edit else model).read(), text_encoder.read())


@app.entrypoint(accelerator=False, defaults={"model": BASE, "other": OTHER}, components={"model": DIT, "other": DIT})
def pair(payload: Ask, model: Transformer, other: Transformer) -> Read:
    """Two models that take LoRAs, both read by every request."""
    return Read(model.read() + other.read())


@app.entrypoint(accelerator=False, defaults={"text_encoder": BASE}, components={"text_encoder": TEXT})
def plain(payload: Ask, text_encoder: Encoder) -> Read:
    """A model that takes no LoRA."""
    return Read(0.0, text_encoder.read())


@app.entrypoint(accelerator=False, defaults={"lora": PROJ}, needs={"lora": "request.styled"})
async def handed(payload: Hand, lora: Weights) -> Read:
    """`encoded` called with this slot's LoRA on its model when `styled`, else without: the slot's default, or the
    request's `model.lora=` in its place."""
    models = {"model": ModelChoice(lora=(Lora(lora.ref),))} if payload.styled else None
    return await call(encoded, Ask(), models=models)
