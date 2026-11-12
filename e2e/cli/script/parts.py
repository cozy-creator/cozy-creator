# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.45", "msgspec>=0.19"]
# ///
"""The cli suite's composition script (xs-190): `model` reads the `transformer` of e2e/parts, a pipeline of a transformer and
a VAE, and `vae` reads a `vae` from whatever checkpoint names one: the pipeline's own by default, or a model of that component
alone. It says what each slot read (its config's `part` and its weights' sum) and draws them as a picture. Its weights and
configs are e2e/isolated.sh's (`parts`): random tensors, each component's config (`part`, `size`) its file's metadata."""

import struct

import msgspec
import torch
from cozy_runtime.author import App, Config, ImageFrame, Loader, Model, Outputs, uses_components
from cozy_runtime.media import ImageFile

app = App()
PIPELINE = "e2e/parts@0.1.0"


def _section(config: Config, component: str) -> Config:
    """A component's config: its section of a checkpoint of several, the whole config of a checkpoint of it alone."""
    return config.section(component) if component in config.keys() else config


class Part(torch.nn.Module):
    def __init__(self, config: Config, component: str) -> None:
        super().__init__()
        section = _section(config, component)
        self.said = section.as_str("part")
        self.w = torch.nn.Parameter(torch.empty(int(section.as_str("size"))), requires_grad=False)


class Graph:
    def __init__(self, config: Config, component: str) -> None:
        self.components = {component: Part(config, component)}


class Read(msgspec.Struct):
    part: str
    sum: float


class Transformer(Model[Graph]):
    graph: Graph

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: Graph(config, "transformer"))

    @uses_components("transformer")
    def read(self) -> Read:
        part = self.graph.components["transformer"]
        return Read(part.said, float(part.w.double().sum()))


class Vae(Model[Graph]):
    graph: Graph

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: Graph(config, "vae"))

    @uses_components("vae")
    def read(self) -> Read:
        part = self.graph.components["vae"]
        return Read(part.said, float(part.w.double().sum()))


class Ask(msgspec.Struct, kw_only=True):
    label: str = ""


class Composed(msgspec.Struct):
    transformer: Read
    vae: Read
    picture: ImageFile


@app.entrypoint(accelerator=False, defaults={"model": PIPELINE, "vae": PIPELINE},
                components={"model": ("transformer",), "vae": ("vae",)})
def compose(payload: Ask, model: Transformer, vae: Vae, out: Outputs) -> Composed:
    """What `model` and `vae` read, and a picture of it: its bytes are the two sums."""
    transformer, decoder = model.read(), vae.read()
    pixels = struct.pack("<dd", transformer.sum, decoder.sum)
    picture = out.save_image(ImageFrame(16, 1, (pixels * 3)[:48]), format="png")
    return Composed(transformer, decoder, picture)
