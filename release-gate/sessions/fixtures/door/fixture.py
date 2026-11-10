# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.38", "msgspec>=0.19"]
# ///
"""The door session's runs, weightless: outputs whose script would mark itself and read Calcifer's runs if it ran, and a wait
that is a job for as long as it is asked."""

import time

import msgspec
from cozy_runtime.author import App, Outputs
from cozy_runtime.media import File

app = App()
REACH = b"document.documentElement.setAttribute('data-ran', 'yes'); fetch('/v1/runs');"


class Ask(msgspec.Struct, kw_only=True):
    prompt: str = ""


class Hostile(msgspec.Struct):
    page: File
    drawing: File


@app.entrypoint(accelerator=False)
def hostile(payload: Ask, out: Outputs) -> Hostile:
    page = b"<!doctype html><title>hostile</title><script>" + REACH + b"</script>"
    drawing = b'<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><script>' + REACH + b"</script></svg>"
    return Hostile(out.save_bytes(page, media_type="text/html"), out.save_bytes(drawing, media_type="image/svg+xml"))


class Wait(msgspec.Struct, forbid_unknown_fields=True):
    seconds: float


class Waited(msgspec.Struct):
    seconds: float


@app.entrypoint(accelerator=False)
def wait(payload: Wait) -> Waited:
    time.sleep(payload.seconds)
    return Waited(seconds=payload.seconds)
