# The parts of the birefnet package (packages repo) that the release-gate xs132-pipeline fixture calls.
import msgspec
from cozy_runtime.author import App, Context, Model, Outputs, Telemetry
from cozy_runtime.media import ImageFile

class RemoveBackgroundInput(msgspec.Struct):
    image: ImageFile

class RemoveBackgroundOutput(msgspec.Struct):
    image: ImageFile

class BackgroundRemovalModel(Model[object]): ...

app: App

def remove_background(
    ctx: Context, payload: RemoveBackgroundInput, model: BackgroundRemovalModel, out: Outputs, tel: Telemetry
) -> RemoveBackgroundOutput: ...
