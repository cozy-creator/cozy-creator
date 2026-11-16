# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.45", "msgspec>=0.19"]
# ///
"""The cli suite's rule holders (cl-1103): `pick` has two models, each needed by a rule over its request, as Qwen Image 2's two
prompt enhancers are (one with pictures, one without), and reads the one its request needs; with `wrong`, the other, which its rule
ruled out. `route` loads no model and calls `left` or `right` by rule, as H3's `generate` calls one segment function or the other."""

import asyncio

import msgspec

from cozy_runtime.author import App, Loader, Model, Telemetry, call

app = App()


class Extra(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())


class Ask(msgspec.Struct, kw_only=True):
    words: list[str] = []
    upsample: bool = True
    wrong: bool = False
    seconds: float = 0


class Said(msgspec.Struct):
    used: str


_WORDS = "size(request.words) > 0"


@app.entrypoint(
    accelerator=False,
    defaults={"worded": "e2e/rules-worded@0.1.0", "plain": "e2e/rules-plain@0.1.0"},
    needs={"worded": f"request.upsample && {_WORDS}", "plain": f"request.upsample && !({_WORDS})"},
)
def pick(payload: Ask, worded: Extra, plain: Extra, tel: Telemetry) -> Said:
    """Reads the model its rules say the request needs; with `wrong`, the other one."""
    if not payload.upsample:
        return Said(used="")
    first = bool(payload.words) != payload.wrong
    with tel.stage("reading it"):
        _ = (worded if first else plain).graph
    return Said(used="e2e/rules-worded" if first else "e2e/rules-plain")


@app.entrypoint(private=True, accelerator=False, defaults={"extra": "e2e/rules-left@0.1.0"})
def left(payload: Ask, extra: Extra) -> Said:
    return Said(used="e2e/rules-left")


@app.entrypoint(private=True, accelerator=False, defaults={"extra": "e2e/rules-right@0.1.0"})
def right(payload: Ask, extra: Extra) -> Said:
    return Said(used="e2e/rules-right")


@app.entrypoint(accelerator=False, needs={"left": _WORDS, "right": f"!({_WORDS})"})
async def route(payload: Ask) -> Said:
    """Holds `seconds` while its prefetch runs, then calls `left` for a request with words, else `right`."""
    await asyncio.sleep(payload.seconds)
    if payload.words:
        return await call(left, Ask())
    return await call(right, Ask())
