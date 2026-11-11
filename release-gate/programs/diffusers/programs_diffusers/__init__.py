"""The environment of the library programs that need Diffusers (`programs.sh --env diffusers`): its TensorFS (varena with it) and Torch are
the ones the machine installed for a package, which is what ships. Running `ready` makes the machine build it."""

import importlib.metadata

import msgspec

from cozy_runtime.author import App, Context

app = App()


class Ready(msgspec.Struct, frozen=True):
    tensorfs: str
    torch: str


@app.entrypoint(accelerator=False)
async def ready(ctx: Context) -> Ready:
    """The versions this environment holds."""
    return Ready(importlib.metadata.version("tensorfs"), importlib.metadata.version("torch"))
