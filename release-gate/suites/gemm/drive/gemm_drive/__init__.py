"""The GEMM suite's driver (xs-177): each function calls the Runtime's GEMM bench, a built-in of the Runtime's own App, and
returns its measurements. Public API only: the bench runs inside the Runtime, where its kernels live."""

import msgspec

# The pinned typecheck Runtime (0.45.0) predates the bench: drop the ignore when the pin moves past it.
from cozy_runtime import bench  # type: ignore[attr-defined]
from cozy_runtime.author import App, Context, call

app = App()


class Measured(msgspec.Struct, frozen=True):
    #: the bench's measurements, as JSON
    detail: str


@app.entrypoint(accelerator=False)
async def gemm_speed(ctx: Context, *, lanes: str = "", shapes: str = "", seed: int = 177) -> Measured:
    """Every GEMM candidate of each lane at our packages' shapes, interleaved, beside the rule's pick."""
    return Measured((await call(bench.gemm_speed, lanes=lanes, shapes=shapes, seed=seed)).detail)


@app.entrypoint(accelerator=False)
async def gemm_tiles(ctx: Context, *, seed: int = 177) -> Measured:
    """nvfp4's Triton decode and GEMV over the tile settings that keep every output bit."""
    return Measured((await call(bench.gemm_tiles, seed=seed)).detail)


@app.entrypoint(accelerator=False)
async def host_calibrate(ctx: Context) -> Measured:
    """This pod's sustained bf16 GEMM TFLOPS and memory GB/s, held against its card's reference (calibrate.sh)."""
    return Measured((await call(bench.host_calibrate)).detail)
