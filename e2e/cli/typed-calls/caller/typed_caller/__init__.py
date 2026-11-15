"""CPU caller: no child implementation or xxhash; the machine may supply its shared Torch layer."""

import importlib.util
import sys
import msgspec
from cozy_runtime.author import (
    App,
    ChildCallError,
    ModelChoice,
    ImageFrame,
    call,
)
from cozy_runtime.media import Files, ImageFile
from .child_calls import (
    Request,
    WeightRequest,
    shade,
    weighted,
    shade_assets,
    shade_assetsRequest,
)

app = App()


class Ask(msgspec.Struct, kw_only=True):
    fail: bool = False
    choice: str = ""
    use_assets: bool = False


class Answer(msgspec.Struct, kw_only=True):
    image: ImageFile | None = None
    prime: bool = False
    error: str = ""
    child_request: str = ""
    python: str = ""
    child_python: str = ""
    descendant_python: str = ""
    value: float = 0.0
    checkpoint: str = ""
    child_installed: bool = False
    callee_dependency_installed: bool = False
    shared_torch_available: bool = False
    shared_sympy_available: bool = False
    torch_imported: bool = False
    fingerprint: str = ""
    squared: int = 0


@app.entrypoint(accelerator=False)
async def run(payload: Ask) -> Answer:
    child_installed = importlib.util.find_spec("typed_child") is not None
    callee_dependency_installed = importlib.util.find_spec("xxhash") is not None
    shared_torch_available = importlib.util.find_spec("torch") is not None
    shared_sympy_available = importlib.util.find_spec("sympy") is not None
    if payload.choice:
        selected = await call(
            weighted,
            WeightRequest(),
            models={"weights": ModelChoice(ref=payload.choice)},
        )
        return Answer(
            python=sys.executable,
            child_python=selected.python,
            value=selected.value,
            checkpoint=selected.checkpoint,
            child_installed=child_installed,
            callee_dependency_installed=callee_dependency_installed,
            shared_torch_available=shared_torch_available,
            shared_sympy_available=shared_sympy_available,
            torch_imported="torch" in sys.modules,
        )
    image = ImageFile.from_image(
        ImageFrame(4, 3, bytes([15, 63, 127]) * 12), format="png"
    )
    try:
        if payload.use_assets:
            result = await call(
                shade_assets,
                shade_assetsRequest(fail=payload.fail),
                assets=Files([image]),
            )
        else:
            result = await call(shade, Request(image=image, fail=payload.fail))
    except ChildCallError as error:
        return Answer(
            error=error.code,
            child_request=error.child_request_id,
            python=sys.executable,
            child_installed=child_installed,
            callee_dependency_installed=callee_dependency_installed,
            shared_torch_available=shared_torch_available,
            shared_sympy_available=shared_sympy_available,
            torch_imported="torch" in sys.modules,
        )
    return Answer(
        image=result.image,
        prime=result.prime,
        python=sys.executable,
        child_python=result.python,
        descendant_python=result.descendant_python,
        fingerprint=result.fingerprint,
        squared=result.squared,
        child_installed=child_installed,
        callee_dependency_installed=callee_dependency_installed,
        shared_torch_available=shared_torch_available,
        shared_sympy_available=shared_sympy_available,
        torch_imported="torch" in sys.modules,
    )
