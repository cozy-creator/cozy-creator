# The birefnet package (packages repo) as a dependent package sees it: the Runtime's interface wheel, whose
# remove_background takes the request's fields by name and returns a PendingCall (release-gate xs132-pipeline).
import msgspec
from cozy_runtime.author import ModelChoice, PendingCall
from cozy_runtime.media import ImageFile

class RemoveBackgroundInput(msgspec.Struct):
    image: ImageFile

class RemoveBackgroundOutput(msgspec.Struct):
    image: ImageFile

def remove_background(*, image: ImageFile, model: ModelChoice | None = None) -> PendingCall[RemoveBackgroundOutput]: ...
