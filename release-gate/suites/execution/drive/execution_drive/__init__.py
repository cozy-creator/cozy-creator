"""The execution suite's driver (xs-174): each function calls one of the Runtime's own exercisers, built-ins of the
Runtime's App (`cozy_runtime.bench`), and returns its document. Public API only: what needs the Runtime's internals
runs inside the Runtime."""

import msgspec

# The pinned typecheck Runtime (0.45.0) predates the bench: drop the ignore when the pin moves past it.
from cozy_runtime import bench  # type: ignore[attr-defined]
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


@app.entrypoint(accelerator=False)
async def overlay_replay(ctx: Context, *, seed: int = 331, variant: str = "replay") -> Report:
    """Qwen-Image-Edit-2511's DiT with the Lightning LoRA armed as packages arm it, through the Runtime's graphs against
    graphs bypassed; fails when they differ."""
    return Report((await call(bench.overlay_replay, payload=bench.OverlayProbe(seed=seed, variant=variant))).detail)


@app.entrypoint(accelerator=False)
async def lora_overlay_refill(ctx: Context, *, seed: int = 4242) -> Report:
    """The LoRA armed in its DiT's own forward through baked int8 pages under a ballast; fails when a warm call differs
    from its reference."""
    return Report((await call(bench.lora_overlay_refill, payload=bench.RefillProbe(seed=seed))).detail)


@app.entrypoint(accelerator=False)
async def mps_residency(ctx: Context, *, seed: int = 103) -> Report:
    """MPS checkpoint loading, an escaped alias and a refill through the Runtime's residency, on an Apple GPU (no suite
    runs it while none has an Apple GPU)."""
    return Report((await call(bench.mps_residency, seed=seed)).detail)
