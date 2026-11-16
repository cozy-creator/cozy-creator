# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]
# ///
"""The cli suite's orchestrators (cl-1077): `edit` loads no model and calls `used` (e2e/calls-used), never `unused`
(e2e/calls-unused), as H3's `edit` calls its segments and never `upscale`'s regenerate. `adapt` calls `adapted` with a LoRA
(e2e/calls-lora) on its `base` (e2e/calls-base) beside its own `extra` (e2e/calls-other), as H3's `edit` puts its edit LoRA on a
segment's base beside the turbo LoRA."""

import asyncio

import msgspec

from cozy_runtime.author import App, Loader, Lora, Model, ModelChoice, call

app = App()


class Extra(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())


class Ask(msgspec.Struct, kw_only=True):
    seconds: float = 0


class Said(msgspec.Struct):
    used: str


@app.entrypoint(private=True, accelerator=False, defaults={"extra": "e2e/calls-used@0.1.0"})
def used(payload: Ask, extra: Extra) -> Said:
    return Said(used="e2e/calls-used")


@app.entrypoint(private=True, accelerator=False, defaults={"extra": "e2e/calls-unused@0.1.0"})
def unused(payload: Ask, extra: Extra) -> Said:
    return Said(used="e2e/calls-unused")


@app.entrypoint(accelerator=False)
async def edit(payload: Ask) -> Said:
    """Holds `seconds` while its prefetch runs, then calls `used`."""
    await asyncio.sleep(payload.seconds)
    return await call(used, Ask())


@app.entrypoint(private=True, accelerator=False, defaults={"base": "e2e/calls-base@0.1.0", "extra": "e2e/calls-other@0.1.0"})
def adapted(payload: Ask, base: Extra, extra: Extra) -> Said:
    return Said(used="e2e/calls-base under e2e/calls-lora, and e2e/calls-other")


@app.entrypoint(accelerator=False)
async def adapt(payload: Ask) -> Said:
    """Calls `adapted` with e2e/calls-lora on its base."""
    return await call(adapted, Ask(), models={"base": ModelChoice(lora=(Lora("e2e/calls-lora", 1.0),))})
