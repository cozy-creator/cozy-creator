# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.38,<0.43.1", "msgspec>=0.19"]
# ///
"""The project cell's hostile outputs, weightless: a page and a drawing whose script would ask for Calcifer's runs if it ran."""

import msgspec
from cozy_runtime.author import App, File, Outputs

app = App()
REACH = b"fetch('/v1/runs');"


class Ask(msgspec.Struct, kw_only=True):
    prompt: str = ""


class Hostile(msgspec.Struct):
    page: File
    drawing: File


@app.entrypoint()
def hostile(payload: Ask, out: Outputs) -> Hostile:
    page = b"<!doctype html><title>hostile</title><script>" + REACH + b"</script>"
    drawing = b'<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><script>' + REACH + b"</script></svg>"
    return Hostile(out.save_bytes(page, media_type="text/html"), out.save_bytes(drawing, media_type="image/svg+xml"))
