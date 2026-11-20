"""kernel-ready-cpu's drive (xs-213): the Runtime's kernel-readiness exerciser, a built-in of its App
(`cozy_runtime.bench.kernel_ready`), on the CPU. Public API only: the registry's internals run inside the Runtime."""

import msgspec
from cozy_runtime import bench
from cozy_runtime.author import App, Context, call

app = App()


class Report(msgspec.Struct, frozen=True):
    #: the exerciser's document, as JSON
    detail: str


@app.entrypoint(accelerator=False)
async def kernel_ready(ctx: Context, *, mode: str, seed: int = 202611200201) -> Report:
    """One mode of kernel readiness publication under real openers, on the CPU; fails when it publishes wrong."""
    return Report((await call(bench.kernel_ready, bench.KernelReadyRequest(mode=mode, seed=seed))).detail)
