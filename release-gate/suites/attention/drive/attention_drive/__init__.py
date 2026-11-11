"""The attention suite's driver (xs-171): each function calls the Runtime's attention bench, a built-in of the Runtime's
own App, and returns its measurements. Public API only: the bench runs inside the Runtime, where its kernels live."""

import msgspec

# The pinned typecheck Runtime (0.45.0) predates the bench: drop the ignore when the pin moves past it.
from cozy_runtime import bench  # type: ignore[attr-defined]
from cozy_runtime.author import App, Context, call

app = App()


class Measured(msgspec.Struct, frozen=True):
    #: the bench's measurements, as JSON (collect.py)
    detail: str


@app.entrypoint(accelerator=False)
async def attention_inventory(ctx: Context) -> Measured:
    """This card's hardware class, its attention kernels, and the sets of them that can share one process."""
    return Measured((await call(bench.attention_inventory)).detail)


@app.entrypoint(accelerator=False)
async def attention_speed(ctx: Context, *, kernels: str = "", points: str = "", seed: int = 171) -> Measured:
    """`kernels` (comma-separated) timed over the grid, interleaved; SDPA joins every set."""
    return Measured((await call(bench.attention_speed, kernels=kernels, points=points, seed=seed)).detail)


@app.entrypoint(accelerator=False)
async def attention_quality(ctx: Context, *, kernel: str, seed: int = 171) -> Measured:
    """`kernel`'s worst-head error against FP32 attention on every site."""
    return Measured((await call(bench.attention_quality, kernel=kernel, seed=seed)).detail)
