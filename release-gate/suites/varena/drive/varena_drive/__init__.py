"""The varena suite's driver (xs-174): the memory authority's rules that only the Runtime's own varena adapter reaches,
and `vt.offload` inside an executor, are built-ins of the Runtime's App (`cozy_runtime.bench`). Each function calls one
and returns its document. Public API only: the rules varena's public API reaches are the suite's programs."""

import msgspec

# The pinned typecheck Runtime (0.45.0) predates the bench: drop the ignore when the pin moves past it.
from cozy_runtime import bench  # type: ignore[attr-defined]
from cozy_runtime.author import App, Context, call

app = App()


class Report(msgspec.Struct, frozen=True):
    #: the exerciser's document, as JSON
    detail: str


@app.entrypoint(accelerator=False)
async def core(ctx: Context, *, case: str) -> Report:
    """`case`, one of varena's rules that only the Runtime's adapter reaches, in a fresh process; fails when it does."""
    return Report((await call(bench.varena_core, case=case)).detail)


@app.entrypoint(accelerator=False)
async def offload_in_executor(ctx: Context) -> Report:
    """`vt.offload` inside an executor, on its allocator and stream; fails when the offloaded model serves otherwise."""
    return Report((await call(bench.offload_in_executor)).detail)
