"""CPU caller: typed references import no child implementation or SymPy."""
import importlib.util
import sys
import msgspec
from cozy_runtime.author import App, ChildCallError, ModelChoice, Outputs, ImageFrame, call
from cozy_runtime.media import ImageFile
from .child_calls import Request, WeightRequest, shade, weighted

app = App()

class Ask(msgspec.Struct, kw_only=True):
    fail: bool = False
    choice: str = ""

class Answer(msgspec.Struct, kw_only=True):
    image: ImageFile | None = None
    prime: bool = False
    error: str = ""
    child_request: str = ""
    python: str = ""
    child_python: str = ""
    descendant_python: str = ""
    value: float = 0
    checkpoint: str = ""
    child_installed: bool = False
    compute_installed: bool = False

@app.entrypoint(accelerator=False)
async def run(payload: Ask, out: Outputs) -> Answer:
    child_installed = importlib.util.find_spec("typed_child") is not None
    compute_installed = importlib.util.find_spec("sympy") is not None
    if payload.choice:
        result = await call(weighted, WeightRequest(), weights=ModelChoice(ref=payload.choice))
        return Answer(python=sys.executable, child_python=result.python, value=result.value,
                      checkpoint=result.checkpoint, child_installed=child_installed, compute_installed=compute_installed)
    image = out.save_image(ImageFrame(4, 3, bytes([15, 63, 127]) * 12), format="png")
    try:
        result = await call(shade, Request(image=image, fail=payload.fail))
    except ChildCallError as error:
        return Answer(error=error.code, child_request=error.child_request_id, python=sys.executable,
                      child_installed=child_installed, compute_installed=compute_installed)
    return Answer(image=result.image, prime=result.prime, python=sys.executable, child_python=result.python, descendant_python=result.descendant_python,
                  child_installed=child_installed, compute_installed=compute_installed)
