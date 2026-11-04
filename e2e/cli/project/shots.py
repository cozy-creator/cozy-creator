# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.38", "msgspec>=0.19"]
# ///
"""The project cell's shot package: weightless, an adapter per kind, and each function says what it was given. `shot`: its video
is its plate, `given` the request it got, and `written` a picture per write. `bare` takes no writes. `typed` types its operation's
fields. `two calls` answers a list of calls, the second reading the first's output. `slow` waits, so a render can be caught going."""

import hashlib
import json
import os
import time

import msgspec
from cozy_runtime.author import App, File, ImageFrame, Invocation, Operation, Outputs, Refuse

app = App()


class Plate(msgspec.Struct, kw_only=True):
    video: File


class Keeping(msgspec.Struct, kw_only=True):
    video: File
    latents: File | None = None
    at_s: float | None = None


class Ask(msgspec.Struct, kw_only=True):
    name: str
    at_s: float | None = None


class Request(msgspec.Struct, kw_only=True):
    prompt: str
    plate: Plate
    seed: int | None = None
    writes: list[Ask] = []
    continue_from: Keeping | None = None


class Take(msgspec.Struct):
    video: File
    latents: File
    given: File
    written: list[File]
    kept_s: float | None
    warnings: list[str]


@app.entrypoint()
def shoot(payload: Request, out: Outputs) -> Take:
    keeps = payload.continue_from
    given = {"prompt": payload.prompt, "seed": payload.seed, "writes": [ask.name for ask in payload.writes],
             "at": [ask.at_s for ask in payload.writes], "keep": keeps.at_s if keeps else None}
    frames = [out.save_image(ImageFrame(16, 16, os.urandom(3) * 256), format="png") for _ in payload.writes]
    return Take(out.save_bytes(payload.plate.video.read_bytes(), media_type="video/mp4"), out.save_bytes(os.urandom(32), media_type="application/octet-stream"),
                out.save_bytes(json.dumps(given).encode(), media_type="application/json"), frames,
                (keeps.at_s or 0) + 0.04 if keeps else None, [])


@app.project_adapter("shot")
def adapt(op: Operation) -> list[Invocation]:
    """Its own words, nothing the format names. A field says when to answer no run, or to refuse."""
    if op.fields.get("refuse"):
        raise Refuse(str(op.fields["refuse"]))
    plate = next((read for read in op.reads if "video" in read.files), None)
    if plate is None or op.fields.get("no_run"):
        return []
    said = [f"{read.name}={read.kind}" for read in op.reads]
    told = " | ".join([op.asset, str(op.fields.get("summary", "")), *said])
    request = {"prompt": told, "plate": {"video": plate.files["video"].sha256}, "writes": [{"name": write.name, "at_s": write.at_s} for write in op.writes]}
    if op.keep:
        request["continue_from"] = {"video": op.keep.video.sha256, "at_s": op.keep.at_s} | ({"latents": op.keep.latents.sha256} if op.keep.latents else {})
    return [Invocation(function="shoot", input=request)]


class Bare(msgspec.Struct, kw_only=True):
    prompt: str
    plate: File


class BareTake(msgspec.Struct):
    video: File


@app.entrypoint()
def bare(payload: Bare, out: Outputs) -> BareTake:
    return BareTake(out.save_bytes(payload.plate.read_bytes(), media_type="video/mp4"))


@app.project_adapter("bare")
def adapt_bare(op: Operation) -> list[Invocation]:
    plate = next(read for read in op.reads if "video" in read.files)
    return [Invocation(function="bare", input={"prompt": str(op.fields.get("summary", "")), "plate": plate.files["video"].sha256})]


class Words(msgspec.Struct, kw_only=True):
    summary: str = ""


class TypedOperation(Operation, kw_only=True):
    fields: Words


class Text(msgspec.Struct, kw_only=True):
    prompt: str


class Saved(msgspec.Struct):
    given: File


@app.entrypoint()
def typed(payload: Text, out: Outputs) -> Saved:
    return Saved(out.save_bytes(payload.prompt.encode() + os.urandom(4), media_type="application/json"))


@app.project_adapter("typed")
def adapt_typed(op: TypedOperation) -> list[Invocation]:
    return [Invocation(function="typed", input={"prompt": op.fields.summary})]


class First(msgspec.Struct):
    first: File


class Second(msgspec.Struct):
    second: File


class Earlier(msgspec.Struct, kw_only=True):
    of: File


@app.entrypoint()
def first_call(payload: Text, out: Outputs) -> First:
    return First(out.save_bytes(payload.prompt.encode() + os.urandom(4), media_type="application/json"))


@app.entrypoint()
def second_call(payload: Earlier, out: Outputs) -> Second:
    return Second(out.save_bytes(f"sha256:{hashlib.sha256(payload.of.read_bytes()).hexdigest()}".encode(), media_type="text/plain"))


@app.project_adapter("two calls")
def adapt_two(op: Operation) -> list[Invocation]:
    return [Invocation(function="first_call", input={"prompt": op.asset}), Invocation(function="second_call", input={"of": "$0.first"})]


class Wait(msgspec.Struct, kw_only=True):
    seconds: float = 20
    prompt: str = ""


@app.entrypoint()
def slow(payload: Wait, out: Outputs) -> Saved:
    time.sleep(payload.seconds)
    return Saved(out.save_bytes(os.urandom(8), media_type="application/json"))


@app.project_adapter("slow")
def adapt_slow(op: Operation) -> list[Invocation]:
    return [Invocation(function="slow", input={"seconds": op.fields.get("seconds", 20)})]
