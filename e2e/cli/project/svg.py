# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""web/access.js's picture with script in it (cl-985): an SVG whose <script> and onload each ask for a marked URL, which must never be
asked: Calcifer sends it sandboxed, with `script-src 'none'`."""

import msgspec
from cozy_runtime.author import App, Outputs
from cozy_runtime.media import File

app = App()

SVG = (
    b'<svg xmlns="http://www.w3.org/2000/svg" width="64" height="48" onload="fetch(\'/v1/status?svg=onload\')">'
    b'<rect width="64" height="48" fill="#c33"/><script>fetch("/v1/status?svg=script")</script></svg>'
)


class Nothing(msgspec.Struct, kw_only=True):
    pass


class Drawn(msgspec.Struct):
    picture: File


@app.entrypoint()
def svg(payload: Nothing, out: Outputs) -> Drawn:
    """A picture whose script must never run."""
    return Drawn(out.save_bytes(SVG, media_type="image/svg+xml"))
