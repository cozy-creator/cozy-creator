"""The cli suite's package that carries its model's tokenizer file (disk_run_object, cl-1076), as minimax-h3 carries the
Qwen3-VL processor files H3's checkpoint holds: `processor/vocab.json` here is also an asset of e2e/disk-tokenizer
(e2e/isolated.sh's `disk`). `read` builds that model, 384 MiB of random weights, and says the text it was given."""

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


@app.entrypoint(accelerator=False, defaults={"weights": "e2e/disk-tokenizer@0.1.0"})
def read(payload: Read, weights: Weights) -> Done:
    """Builds its model and says the text it was given."""
    _ = weights.graph
    return Done(payload.text)
