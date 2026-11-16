# The birefnet implementation signature for ordinary local use. Managed callers use generated Operation references
# and `await call(remove_background, RemoveBackgroundInput(...))` (release-gate xs132-pipeline).
import msgspec
from cozy_runtime.author import Context, Telemetry
from cozy_runtime.media import ImageFile

class RemoveBackgroundInput(msgspec.Struct):
    image: ImageFile

class RemoveBackgroundOutput(msgspec.Struct):
    image: ImageFile

class BackgroundRemovalModel: ...

def remove_background(
    ctx: Context, payload: RemoveBackgroundInput, model: BackgroundRemovalModel, tel: Telemetry
) -> RemoveBackgroundOutput: ...
