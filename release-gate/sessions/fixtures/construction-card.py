# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.48.0.dev0,<0.49", "msgspec>=0.19"]
# ///
"""construction-card's function (cr-335, D3): what `torch.cuda` tells a model's construction and the request after it, in
one executor. The model's factory reads the card and builds nothing else (a small VAE's slot, never read)."""

import importlib.metadata

import msgspec

from cozy_runtime.author import App, Loader, Model

app = App()


class Card(msgspec.Struct):
    count: int
    name: str
    capability: tuple[int, int]
    available: bool


def read() -> Card:
    import torch

    major, minor = torch.cuda.get_device_capability()
    return Card(torch.cuda.device_count(), torch.cuda.get_device_name(), (major, minor), torch.cuda.is_available())


class Built(Model[Card]):
    seen: Card

    def load(self, loader: Loader) -> None:
        self.seen = loader.construct(lambda config: read())


class Ask(msgspec.Struct, kw_only=True):
    note: str = ""


class Seen(msgspec.Struct):
    runtime: str
    construction: Card
    request: Card


@app.entrypoint(accelerator=True, defaults={"built": "madebyollin/texture-fix-vae@0.3.0"})
def card(payload: Ask, built: Built) -> Seen:
    return Seen(importlib.metadata.version("cozy-runtime"), built.seen, read())
