"""ggml-kernels' driver (xs-213): the Runtime's GEMM bench at the ggml lane, its batch-shapes, the pod's copy
bandwidth, and the Runtime its environment imports. Public API only: the bench runs inside the Runtime, where its kernels live."""

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
async def batch_shapes(ctx: Context) -> Measured:
    """batch-shapes raced on torch's default stream with no sync, beside the same cases settled (cozy-runtime#2309)."""
    return Measured((await call(bench.ggml_batch_shapes, bench.EmptyRequest())).detail)


@app.entrypoint(accelerator=False)
async def runtime_version(ctx: Context) -> Measured:
    """The cozy-runtime this environment imports, which the session holds against the candidate wheel."""
    import cozy_runtime

    return Measured(cozy_runtime.__version__)


@app.entrypoint(accelerator=False)
async def ggml_probe(ctx: Context) -> Measured:
    """The ggml matmuls' probe as a card runs it before its ggml leaves serve (dense rows, MUL_MAT_ID over an expert
    table and a stack): the verdict, each case's error and a MUL_MAT_ID case's non-finite rows (xs-213)."""
    return Measured((await call(bench.ggml_probe, bench.EmptyRequest())).detail)
