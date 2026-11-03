"""A parent gives its own saved files to its child (cr-251): the child reads the bytes the parent saved. Everything else
stays refused: a file of another attempt, and a call whose wire lies about what the parent holds, as a package could."""

from __future__ import annotations

import hashlib
import io
import os
from typing import Annotated

import msgspec
from PIL import Image

from cozy_runtime.author import App, Context, File, FileBound, ImageFile, Outputs, invocable
from cozy_runtime.author._calls import ChildCallError, _current
from cozy_runtime.author._errors import CapabilityError
from cozy_runtime.author._executor_requests import CallState, ChildCall, SavedFile

app = App()
TREE = "application/vnd.cozy.tree-manifest"
#: A file an earlier attempt of this executor saved, if one ran here.
EARLIER: list[File] = []


class Picture(msgspec.Struct, kw_only=True):
    picture: Annotated[ImageFile, FileBound(max_bytes=1 << 20)]


class Blob(msgspec.Struct, kw_only=True):
    file: Annotated[File, FileBound(max_bytes=1 << 20)]


class Seen(msgspec.Struct):
    digest: str
    size: int
    media_type: str
    width: int = 0


def sha(data: bytes) -> str:
    return f"sha256:{hashlib.sha256(data).hexdigest()}"


@invocable()
async def see_picture(ctx: Context, *, payload: Picture) -> Seen:
    data = payload.picture.read_bytes()
    return Seen(sha(data), len(data), payload.picture.media_type, Image.open(io.BytesIO(data)).width)


app.child_task(see_picture)


@invocable()
async def see_blob(ctx: Context, *, payload: Blob) -> Seen:
    data = payload.file.read_bytes()
    return Seen(sha(data), len(data), payload.file.media_type)


app.child_task(see_blob)


class Nothing(msgspec.Struct):
    pass


class Result(msgspec.Struct):
    image: ImageFile
    saved_picture: str
    seen_picture: Seen
    saved_blob: str
    seen_blob: Seen
    #: Each refusal's code; "" had it been let through.
    other_attempt: str
    earlier_attempt: str
    ungranted: str
    lying_digest: str
    claimed_tree: str
    symlink: str


async def code_of(call) -> str:
    try:
        await call()
    except (CapabilityError, ChildCallError) as refused:
        return refused.code
    return ""


def wire_code(saved: SavedFile) -> str:
    """The call a package could send itself: TensorD alone stands between it and the file."""
    answer = _current.get().exchange(
        ChildCall(call_index=900, module=__name__, export="see_blob", payload="{}", saved=(saved,)), CallState
    )
    return "" if answer.ok else answer.code


async def main(ctx: Context, payload: Nothing, out: Outputs) -> Result:
    image = out.save_image(Image.new("RGB", (64, 48), (200, 40, 40)))
    seen_picture = await see_picture(payload=Picture(picture=image))
    data = os.urandom(4096)
    blob = out.save_bytes(data, media_type="application/octet-stream")
    seen_blob = await see_blob(payload=Blob(file=blob))

    foreign = File("attempt:elsewhere/file/0001", media_type=blob.media_type, size_bytes=len(data), digest=blob.digest,
                   local=blob._local, attempt="elsewhere")
    earlier = EARLIER[-1] if EARLIER else None
    EARLIER.append(blob)
    spool = blob._local.parent
    (spool / "file-link").symlink_to("/etc/hostname")
    (spool / "file-tree").write_bytes(b"a tree's manifest names other objects")
    with open("/etc/hostname", "rb") as secret:
        hostname = sha(secret.read())
    return Result(
        image=image,
        saved_picture=sha(image._local.read_bytes()),
        seen_picture=seen_picture,
        saved_blob=sha(data),
        seen_blob=seen_blob,
        other_attempt=await code_of(lambda: see_blob(payload=Blob(file=foreign))),
        earlier_attempt="none here" if earlier is None else await code_of(lambda: see_blob(payload=Blob(file=earlier))),
        ungranted=await code_of(lambda: see_blob(payload=Blob(file=File(sha(b"the owner's other file"))))),
        lying_digest=wire_code(SavedFile(asset_ref=blob.ref, digest=sha(b"not these bytes"), media_type=blob.media_type)),
        claimed_tree=wire_code(SavedFile(
            asset_ref="attempt:x/file/tree", digest=sha(b"a tree's manifest names other objects"), media_type=TREE
        )),
        symlink=wire_code(SavedFile(asset_ref="attempt:x/file/link", digest=hostname, media_type="text/plain")),
    )


app.entrypoint(main, accelerator=False)
