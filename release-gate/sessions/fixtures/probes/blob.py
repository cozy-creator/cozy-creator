# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.30", "msgspec>=0.19"]
# ///
"""A weightless run for the WebRTC probe: one large output to read in bulk, one tiny output to read
small, both deterministic zeros so no content is secret. No accelerator."""

import msgspec
from cozy_runtime.author import App, File, Outputs

app = App()


class Ask(msgspec.Struct, kw_only=True):
    mib: int = 100


class Made(msgspec.Struct):
    blob: File
    small: File


@app.entrypoint(accelerator=False)
def blob(payload: Ask, out: Outputs) -> Made:
    big = bytes(payload.mib * 1024 * 1024)
    return Made(
        out.save_bytes(big, media_type="application/octet-stream"),
        out.save_bytes(b"\0" * 4096, media_type="application/octet-stream"),
    )
