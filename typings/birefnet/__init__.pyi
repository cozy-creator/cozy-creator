# The birefnet package (packages repo) as a dependent package imports it: its registered function, which a caller
# runs as a managed call (`await call(remove_background, payload=...)`, release-gate xs132-pipeline).
import msgspec
from cozy_runtime.author import Context, Outputs, Telemetry
from cozy_runtime.media import ImageFile

class RemoveBackgroundInput(msgspec.Struct):
    image: ImageFile

class RemoveBackgroundOutput(msgspec.Struct):
    image: ImageFile

class BackgroundRemovalModel: ...

def remove_background(
    ctx: Context, payload: RemoveBackgroundInput, model: BackgroundRemovalModel, out: Outputs, tel: Telemetry
) -> RemoveBackgroundOutput: ...
