"""estimate_plan's sized function (cl-1286, e2e/cli/estimate.sh): no model, so it runs on any computer; its estimate names
its size variables, one of which (`frames`) gives no value without a clip."""

import msgspec

from cozy_runtime.author import App

app = App()


class Clip(msgspec.Struct, kw_only=True):
    frames: int


class Shot(msgspec.Struct, kw_only=True):
    duration: float = 5.0
    width: int = 1344
    height: int = 768
    turbo: bool = True
    clip: Clip | None = None


class Made(msgspec.Struct):
    seconds: float


_SIZE = {
    "seconds": "request.duration",
    "megapixels": "double(request.width * request.height) / 1e6",
    "steps": "request.turbo ? 8.0 : 30.0",
    "frames": "double(request.clip.frames)",
}


@app.entrypoint(accelerator=False, estimate={"size": _SIZE, "ms": "1000.0 * size.seconds * size.steps / 8.0"})
def sized(payload: Shot) -> Made:
    return Made(seconds=payload.duration)
