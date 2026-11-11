"""The kernels suite's driver (xs-174): each function calls one of the Runtime's kernel exercisers, built-ins of the
Runtime's own App (`cozy_runtime.bench`), and returns its document. Public API only: the exercisers run inside the
Runtime, where its kernels live, and a package can't import its internals."""

import msgspec

from cozy_runtime import bench
from cozy_runtime.author import App, Context, call

app = App()


class Report(msgspec.Struct, frozen=True):
    #: the exerciser's document, as JSON (check.py)
    detail: str


@app.entrypoint(accelerator=False)
async def inventory(ctx: Context) -> Report:
    """Every kernel this card claims, read from the Runtime's code."""
    return Report((await call(bench.kernels_inventory)).detail)


@app.entrypoint(accelerator=False)
async def gpu(ctx: Context, *, kernel: str, index: int = 0, seed: int = 400) -> Report:
    """`kernel` on GPU `index` through its real launch path, against a slow reference; fails when it does."""
    return Report((await call(bench.kernel_check, name=kernel, index=index, seed=seed)).detail)
