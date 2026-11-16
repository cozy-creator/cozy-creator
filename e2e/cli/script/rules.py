# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.47.0.dev0,<0.48", "msgspec>=0.19"]
# ///
"""The cli suite's rule holders (cl-1103): `pick` has two models, each needed by a rule over its request, as Qwen Image 2's two
prompt enhancers are (one with pictures, one without), and reads the one its request needs. `route` loads no model and calls `left`
or `right` by rule, as H3's `generate` calls one segment function or the other. `stray` has `left`'s and `right`'s models under
the same rules and reads the one its rule rules out: a rule wrong for its request. `hinted` has `pick`'s models and says mid-run
which one it will read against its rule (`ctx.need`), holds, then reads it."""

import asyncio
import time

import msgspec

from cozy_runtime.author import App, Context, Loader, Model, Telemetry, call

app = App()


class Extra(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())


class Ask(msgspec.Struct, kw_only=True):
    words: list[str] = []
    upsample: bool = True
    seconds: float = 0


class Said(msgspec.Struct):
    used: str


class Read(msgspec.Struct):
    used: str
    #: How long its read of the model took.
    read_ms: int


_WORDS = "size(request.words) > 0"


@app.entrypoint(
    accelerator=False,
    defaults={"worded": "e2e/rules-worded@0.1.0", "plain": "e2e/rules-plain@0.1.0"},
    needs={"worded": f"request.upsample && {_WORDS}", "plain": f"request.upsample && !({_WORDS})"},
)
def pick(payload: Ask, worded: Extra, plain: Extra, tel: Telemetry) -> Said:
    """Reads the model its rules say the request needs."""
    if not payload.upsample:
        return Said(used="")
    with tel.stage("reading it"):
        _ = (worded if payload.words else plain).graph
    return Said(used="e2e/rules-worded" if payload.words else "e2e/rules-plain")


@app.entrypoint(private=True, accelerator=False, defaults={"extra": "e2e/rules-left@0.1.0"})
def left(payload: Ask, extra: Extra) -> Said:
    return Said(used="e2e/rules-left")


@app.entrypoint(private=True, accelerator=False, defaults={"extra": "e2e/rules-right@0.1.0"})
def right(payload: Ask, extra: Extra) -> Said:
    return Said(used="e2e/rules-right")


@app.entrypoint(
    accelerator=False,
    defaults={"named": "e2e/rules-left@0.1.0", "other": "e2e/rules-right@0.1.0"},
    needs={"named": _WORDS, "other": f"!({_WORDS})"},
)
def stray(payload: Ask, named: Extra, other: Extra, tel: Telemetry) -> Said:
    """Reads the model its rules say the request doesn't need."""
    with tel.stage("reading it"):
        _ = (other if payload.words else named).graph
    return Said(used="e2e/rules-right" if payload.words else "e2e/rules-left")


@app.entrypoint(
    accelerator=False,
    defaults={"worded": "e2e/rules-worded@0.1.0", "plain": "e2e/rules-plain@0.1.0"},
    needs={"worded": _WORDS, "plain": f"!({_WORDS})"},
)
def hinted(ctx: Context, payload: Ask, worded: Extra, plain: Extra, tel: Telemetry) -> Read:
    """Says it will read the model its rules say the request doesn't need, holds `seconds` while it comes, then reads it."""
    ctx.need("plain" if payload.words else "worded")
    with tel.stage("holding"):
        time.sleep(payload.seconds)
    began = time.time()
    with tel.stage("reading it"):
        _ = (plain if payload.words else worded).graph
    return Read(used="e2e/rules-plain" if payload.words else "e2e/rules-worded", read_ms=int((time.time() - began) * 1000))


@app.entrypoint(accelerator=False, needs={"left": _WORDS, "right": f"!({_WORDS})"})
async def route(payload: Ask) -> Said:
    """Holds `seconds` while its prefetch runs, then calls `left` for a request with words, else `right`."""
    await asyncio.sleep(payload.seconds)
    if payload.words:
        return await call(left, Ask())
    return await call(right, Ask())
