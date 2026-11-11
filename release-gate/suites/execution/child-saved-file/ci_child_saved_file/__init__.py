"""A parent gives its own saved files to its child (cr-251), a labeled copy of one too (cr-256, H3 labels its pictures): the
child reads the bytes the parent saved. Everything else stays refused: a file of another attempt, and a call whose wire lies about what the parent holds, as a package could."""

from __future__ import annotations

import hashlib
import io
import os
from typing import Annotated

import msgspec
from PIL import Image

from cozy_runtime.author import App, Context, Outputs, call
from cozy_runtime.fields import FileBound
from cozy_runtime.media import File, ImageFile
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


@app.entrypoint(private=True)
async def see_picture(ctx: Context, *, payload: Picture) -> Seen:
    data = payload.picture.read_bytes()
    return Seen(sha(data), len(data), payload.picture.media_type, Image.open(io.BytesIO(data)).width)


@app.entrypoint(private=True)
async def see_blob(ctx: Context, *, payload: Blob) -> Seen:
    data = payload.file.read_bytes()
    return Seen(sha(data), len(data), payload.file.media_type)


class Nothing(msgspec.Struct):
    pass


class Result(msgspec.Struct):
    image: ImageFile
    saved_picture: str
    seen_picture: Seen
    saved_labeled: str
    seen_labeled: Seen
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
    seen_picture = await call(see_picture, payload=Picture(picture=image))
    # A frame not encoded yet, labeled before it is: the copy never hears of its bytes.
    labeled = out.save_image(Image.new("RGB", (32, 24), (40, 200, 40)))
    seen_labeled = await call(see_picture, payload=Picture(picture=labeled.with_label("Picture 1")))
    data = os.urandom(4096)
    blob = out.save_bytes(data, media_type="application/octet-stream")
    seen_blob = await call(see_blob, payload=Blob(file=blob))

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
        saved_labeled=sha(labeled._local.read_bytes()),
        seen_labeled=seen_labeled,
        saved_blob=sha(data),
        seen_blob=seen_blob,
        other_attempt=await code_of(lambda: call(see_blob, payload=Blob(file=foreign))),
        earlier_attempt="none here" if earlier is None else await code_of(lambda: call(see_blob, payload=Blob(file=earlier))),
        ungranted=await code_of(lambda: call(see_blob, payload=Blob(file=File(sha(b"the owner's other file"))))),
        lying_digest=wire_code(SavedFile(asset_ref=blob.ref, digest=sha(b"not these bytes"), media_type=blob.media_type)),
        claimed_tree=wire_code(SavedFile(
            asset_ref="attempt:x/file/tree", digest=sha(b"a tree's manifest names other objects"), media_type=TREE
        )),
        symlink=wire_code(SavedFile(asset_ref="attempt:x/file/link", digest=hostname, media_type="text/plain")),
    )


app.entrypoint(main, accelerator=False)
