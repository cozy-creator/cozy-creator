"""ggml-kernels' driver (xs-213): the Runtime's GEMM bench at the ggml lane, the pod's copy bandwidth, and
the Runtime its environment imports. Public API only: the bench runs inside the Runtime, where its kernels live."""

import msgspec
from cozy_runtime import bench
from cozy_runtime.author import App, Context, call

app = App()


class Measured(msgspec.Struct, frozen=True):
    #: the bench's measurements, as JSON
    detail: str


@app.entrypoint(accelerator=False)
async def gemm_speed(ctx: Context, *, lanes: str = "ggml", shapes: str = "", seed: int = 213) -> Measured:
    """Each lane's candidates at the shapes named, interleaved, beside the rule's pick."""
    return Measured((await call(bench.gemm_speed, bench.GemmSpeedRequest(lanes=lanes, shapes=shapes, seed=seed))).detail)


@app.entrypoint(accelerator=False)
async def host_calibrate(ctx: Context) -> Measured:
    """This pod's sustained bf16 GEMM TFLOPS and device-memory copy GB/s."""
    return Measured((await call(bench.host_calibrate, bench.EmptyRequest())).detail)


@app.entrypoint(accelerator=False)
async def runtime_version(ctx: Context) -> Measured:
    """The cozy-runtime this environment imports, which the session holds against the candidate wheel."""
    import cozy_runtime

    return Measured(cozy_runtime.__version__)
