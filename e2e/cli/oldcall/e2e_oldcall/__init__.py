"""Runtime 0.46's managed call with files (cl-1160): its `Files` travel inside the callee's request, where 0.47's
`call(..., assets=)` sends them beside it. minimax-h3 0.12.0 calls its reference segments so."""

import hashlib
from importlib.metadata import version

import msgspec

from cozy_runtime.author import App, call
from cozy_runtime.media import File, Files

app = App()


class Nothing(msgspec.Struct, forbid_unknown_fields=True):
    pass


class Seen(msgspec.Struct):
    files: list[str]
    runtime: str


@app.entrypoint(accelerator=False, private=True)
def seen(payload: Nothing, assets: Files[File]) -> Seen:
    """Each file it was given, `label=sha256:<hex>`, and the Runtime it ran on."""
    files = [f"{assets.info(i).label}=sha256:{hashlib.sha256(assets[i].read_bytes()).hexdigest()}" for i in range(len(assets))]
    return Seen(files=files, runtime=version("cozy-runtime"))


@app.entrypoint(accelerator=False)
async def relay(payload: Nothing, assets: Files[File]) -> Seen:
    """Its files, passed on to `seen` by a managed call."""
    # 0.46's call(fn, **arguments); this tree types it against the RC's Runtime.
    return await call(seen, payload=Nothing(), assets=assets)  # type: ignore[call-overload, no-any-return]
