"""The execution suite's driver (xs-174): each function calls one of the Runtime's own exercisers, built-ins of the
Runtime's App (`cozy_runtime.bench`), and returns its document. Public API only: what needs the Runtime's internals
runs inside the Runtime."""

import msgspec

from cozy_runtime import bench
from cozy_runtime.author import App, Context, call

app = App()


class Report(msgspec.Struct, frozen=True):
    #: the exerciser's document, as JSON
    detail: str


@app.entrypoint(accelerator=False)
async def kernel_ready(ctx: Context, *, mode: str, seed: int = 202610270201) -> Report:
    """Kernel readiness publication under two real openers of one kernel, on the CPU; fails when it publishes wrong."""
    return Report((await call(bench.kernel_ready, mode=mode, seed=seed)).detail)


@app.entrypoint(accelerator=False)
async def child_wire(ctx: Context) -> Report:
    """Child calls whose wire lies about what the attempt holds, as a package could send them; fails unless TensorD
    refuses each."""
    return Report((await call(bench.child_wire)).detail)
