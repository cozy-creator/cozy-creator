# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]
# ///
"""The cli suite's disk filler (tfs-150): `read` builds the model its `weights` slot names, whose bytes come to this machine's
store before it starts, and says so. Its models are e2e/isolated.sh's `disk` ones: random weights, 384 MiB each."""

import msgspec

from cozy_runtime.author import App, Loader, Model

app = App()


class Weights(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())


class Read(msgspec.Struct, kw_only=True):
    text: str = "read"


class Done(msgspec.Struct):
    text: str


@app.entrypoint(accelerator=False, defaults={"weights": "e2e/disk-1@0.1.0"})
def read(payload: Read, weights: Weights) -> Done:
    """Builds its model and says the text it was given."""
    _ = weights.graph
    return Done(payload.text)
