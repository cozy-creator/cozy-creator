# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.33,<0.34", "msgspec>=0.19"]
# ///
"""Real CPU file output through normal Runtime custody and Calcifer delivery.

This isolates transport/finalization from model execution. It is not model-quality
qualification and never initializes a GPU. The seed makes output bytes verifiable.
"""

import hashlib
import time
from typing import Annotated

import msgspec
from cozy_runtime.author import App, AssetBound, FileAsset, Outputs, Telemetry

app = App()


class Request(msgspec.Struct, forbid_unknown_fields=True):
    seed: int
    bytes: Annotated[int, msgspec.Meta(ge=1, le=32 << 20)] = 2 << 20


class Result(msgspec.Struct):
    file: Annotated[FileAsset, AssetBound(max_bytes=32 << 20)]
    sha256: str
    source_bytes: int
    generation_ms: float
    write_ms: float
    registration_ms: float


@app.entrypoint(accelerator=False)
def main(payload: Request, out: Outputs, tel: Telemetry) -> Result:
    start = time.perf_counter_ns()
    data = hashlib.shake_256(str(payload.seed).encode("ascii")).digest(payload.bytes)
    digest = hashlib.sha256(data).hexdigest()
    generated = time.perf_counter_ns()
    path = out.temporary_file(suffix=".bin")
    path.write_bytes(data)
    written = time.perf_counter_ns()
    asset = out.save_file(path, media_type="application/octet-stream")
    registered = time.perf_counter_ns()
    generation_ms = (generated - start) / 1e6
    write_ms = (written - generated) / 1e6
    registration_ms = (registered - written) / 1e6
    tel.log(
        "pipeline file source",
        bytes=payload.bytes,
        sha256=digest,
        generation_ms=generation_ms,
        write_ms=write_ms,
        registration_ms=registration_ms,
    )
    return Result(
        asset, digest, payload.bytes, generation_ms, write_ms, registration_ms
    )
