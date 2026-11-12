# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]
# ///
"""The cli suite's lazy-slot script (xs-186): an on-demand model slot whose default no Hub publishes. A request that
never reads it completes; one that reads it is refused, naming the slot, its model and why."""

import msgspec

from cozy_runtime.author import App, Loader, Model, uses_components

app = App()


class Extra(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())

    @uses_components("transformer")
    def forward(self, text: str) -> str:
        return text


class Ask(msgspec.Struct, kw_only=True):
    use: bool = False


class Said(msgspec.Struct):
    used: bool


@app.entrypoint(defaults={"extra": "e2e/absent@0.1.0"}, on_demand=("extra",))
def maybe(payload: Ask, extra: Extra) -> Said:
    """Reads `extra` only when asked."""
    if payload.use:
        extra.forward("used")
    return Said(used=payload.use)
