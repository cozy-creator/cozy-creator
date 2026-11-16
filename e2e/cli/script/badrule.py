# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.47.0.dev0,<0.48", "msgspec>=0.19"]
# ///
"""The cli suite's bad rule (cl-1103): `typo`'s rule reads `request.wordz`, a field its request doesn't have. Cozy refuses it when
it reads the code, before anything installs or runs."""

import msgspec

from cozy_runtime.author import App, Loader, Model

app = App()


class Extra(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())


class Ask(msgspec.Struct, kw_only=True):
    words: list[str] = []


class Said(msgspec.Struct):
    used: str


@app.entrypoint(accelerator=False, defaults={"extra": "e2e/rules-plain@0.1.0"}, needs={"extra": "size(request.wordz) > 0"})
def typo(payload: Ask, extra: Extra) -> Said:
    return Said(used="e2e/rules-plain")
