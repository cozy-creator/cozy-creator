# The parts of the sdxl package (packages repo) that the release-gate fixtures call: pause-twice, child-lora-stacks.
import msgspec
from cozy_runtime.author import Adjustments, App, Context, Model, Outputs, Telemetry
from cozy_runtime.fields import Preflight
from cozy_runtime.image import ImageRequest, ImageResult

class GenerateInput(ImageRequest):
    steps: int | None = None
    guidance: float | None = None
    negative_prompt: str | None = None
    hidiffusion: bool | None = None

class GenerateFacts(msgspec.Struct, frozen=True):
    width: int
    height: int

class SdxlPipeline: ...
class SdxlModel(Model[SdxlPipeline]): ...

app: App

def resolve_generate(payload: GenerateInput) -> GenerateFacts: ...
def generate(
    ctx: Context, payload: GenerateInput, facts: Preflight[GenerateFacts], model: SdxlModel, out: Outputs, tel: Telemetry,
    adj: Adjustments,
) -> ImageResult: ...
