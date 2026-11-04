# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.38", "msgspec>=0.19"]
# ///
"""The project cell's shot package: weightless, an adapter per file type, and each function says what it was given. A `video`: its
video is its plate and `given` the request it got; the field `bare` calls a function of just its video, `two_calls` answers a list of calls, the second reading the first's output, and `seconds` waits, so a render can be caught
going, `fresh` makes its plate's video new bytes, a file nothing has measured, `noise` a video ffprobe can't read, and `crash` a run that fails saying it. An `image`
types its operation's fields. An `audio` says its `options`."""

import hashlib
import json
import os
import struct
import time

import msgspec
from cozy_runtime.author import App, File, Invocation, Operation, Outputs, Refuse

app = App()


class Plate(msgspec.Struct, kw_only=True):
    video: File


class Keeping(msgspec.Struct, kw_only=True):
    video: File
    latents: File | None = None
    at_s: float | None = None


class Request(msgspec.Struct, kw_only=True):
    prompt: str
    plate: Plate
    seed: int | None = None
    continue_from: Keeping | None = None


class Take(msgspec.Struct):
    video: File
    latents: File
    given: File
    kept_s: float | None
    warnings: list[str]


@app.entrypoint()
def shoot(payload: Request, out: Outputs) -> Take:
    keeps = payload.continue_from
    given = {"prompt": payload.prompt, "seed": payload.seed, "keep": keeps.at_s if keeps else None}
    return Take(out.save_bytes(payload.plate.video.read_bytes(), media_type="video/mp4"), out.save_bytes(os.urandom(32), media_type="application/octet-stream"),
                out.save_bytes(json.dumps(given).encode(), media_type="application/json"),
                (keeps.at_s or 0) + 0.04 if keeps else None, [])


@app.project_adapter("video", reserved=["d", "english"])
def adapt(op: Operation) -> list[Invocation]:
    """Its own words, nothing the format names. A field says what to do: answer no run, refuse, or call another of its functions."""
    if op.fields.get("refuse"):
        raise Refuse(str(op.fields["refuse"]))
    if op.fields.get("crash"):
        return [Invocation(function="crash", input={"prompt": str(op.fields["crash"])})]
    if "seconds" in op.fields:
        return [Invocation(function="slow", input={"seconds": op.fields.get("seconds", 20)})]
    if op.fields.get("two_calls"):
        return [Invocation(function="first_call", input={"prompt": op.asset}), Invocation(function="second_call", input={"of": "$0.first"})]
    plate = next((read for read in op.reads if "video" in read.files), None)
    if plate is None or op.fields.get("no_run"):
        return []
    if op.fields.get("bare"):
        return [Invocation(function="bare", input={"prompt": str(op.fields.get("summary", "")), "plate": plate.files["video"].sha256})]
    if op.fields.get("fresh"):
        return [Invocation(function="fresh", input={"plate": plate.files["video"].sha256})]
    if op.fields.get("noise"):
        return [Invocation(function="noise", input={"prompt": op.asset})]
    said = [f"{read.name}={read.kind}{'+latents' if 'latents' in read.files else ''}" for read in op.reads]
    told = " | ".join([op.asset, str(op.fields.get("summary", "")), *said])
    request = {"prompt": told, "plate": {"video": plate.files["video"].sha256}}
    if op.keep:
        request["continue_from"] = {"video": op.keep.video.sha256, "at_s": op.keep.at_s} | ({"latents": op.keep.latents.sha256} if op.keep.latents else {})
    return [Invocation(function="shoot", input=request)]


class Bare(msgspec.Struct, kw_only=True):
    prompt: str
    plate: File


class BareTake(msgspec.Struct):
    video: File


class Fresh(msgspec.Struct, kw_only=True):
    plate: File
    seed: int | None = None


@app.entrypoint()
def bare(payload: Bare, out: Outputs) -> BareTake:
    return BareTake(out.save_bytes(payload.plate.read_bytes(), media_type="video/mp4"))


@app.entrypoint()
def fresh(payload: Fresh, out: Outputs) -> BareTake:
    return BareTake(out.save_bytes(payload.plate.read_bytes() + struct.pack(">I4s", 24, b"free") + os.urandom(16), media_type="video/mp4"))




class Words(msgspec.Struct, kw_only=True):
    summary: str = ""


class TypedOperation(Operation, kw_only=True):
    fields: Words


class Text(msgspec.Struct, kw_only=True):
    prompt: str


@app.entrypoint()
def noise(payload: Text, out: Outputs) -> BareTake:
    return BareTake(out.save_bytes(os.urandom(4096), media_type="video/mp4"))


class Saved(msgspec.Struct):
    given: File


@app.entrypoint()
def crash(payload: Text, out: Outputs) -> Saved:
    raise RuntimeError(payload.prompt)


@app.entrypoint()
def typed(payload: Text, out: Outputs) -> Saved:
    return Saved(out.save_bytes(payload.prompt.encode() + os.urandom(4), media_type="application/json"))


@app.project_adapter("image")
def adapt_typed(op: TypedOperation) -> list[Invocation]:
    return [Invocation(function="typed", input={"prompt": op.fields.summary})]


class Untyped(msgspec.Struct):
    blob: File


@app.entrypoint()
def only_untyped(payload: Text, out: Outputs) -> Untyped:
    return Untyped(out.save_bytes(payload.prompt.encode() + os.urandom(4), media_type="application/octet-stream"))


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




@app.entrypoint()
def echo(payload: Text, out: Outputs) -> Saved:
    return Saved(out.save_bytes(payload.prompt.encode(), media_type="application/json"))


@app.project_adapter("audio")
def adapt_told(op: Operation) -> list[Invocation]:
    """`options` is its own map: what it does with them is its business."""
    return [Invocation(function="echo", input={"prompt": json.dumps(op.options, sort_keys=True)})]


class Wait(msgspec.Struct, kw_only=True):
    seconds: float = 20
    prompt: str = ""


@app.entrypoint()
def slow(payload: Wait, out: Outputs) -> Saved:
    time.sleep(payload.seconds)
    return Saved(out.save_bytes(os.urandom(8), media_type="application/json"))


