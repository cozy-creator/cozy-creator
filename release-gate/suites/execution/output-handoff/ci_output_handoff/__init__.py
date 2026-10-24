"""Real CUDA computation and a raw image consumed by the ordinary frame codec.

Both workload modes use the same entrypoint/construction so a healthy executor is reusable.
The cell does not load a checkpoint or claim model-quality/performance qualification.
"""

import hashlib
import json
import os
import time
from typing import Annotated

import msgspec
from cozy_runtime.author import (
    App,
    AssetBound,
    Context,
    FileAsset,
    ImageAsset,
    ImageFrame,
    Outputs,
    Telemetry,
)

app = App()


class Request(msgspec.Struct, forbid_unknown_fields=True):
    tag: str
    seed: int
    image: bool = False
    size: Annotated[int, msgspec.Meta(ge=512, le=4096)] = 4096
    iterations: Annotated[int, msgspec.Meta(ge=1, le=8192)] = 16
    file_mib: Annotated[int, msgspec.Meta(ge=0, le=240)] = 0


class Result(msgspec.Struct):
    tag: str
    pid: int
    gpu_started_ns: int
    gpu_finished_ns: int
    pixels_sha256: str
    images: Annotated[
        list[ImageAsset],
        AssetBound(
            max_bytes=64 << 20, max_decoded_bytes=64 << 20, media_types=("image/webp",)
        ),
    ]
    files: Annotated[
        list[FileAsset],
        AssetBound(max_bytes=240 << 20, media_types=("application/octet-stream",)),
    ]
    file_sha256: str = ""


@app.entrypoint
def run(payload: Request, ctx: Context, out: Outputs, tel: Telemetry) -> Result:
    import torch

    if ctx.device.type != "cuda":
        raise ValueError("the output-handoff cell requires one CUDA GPU")
    if payload.image and payload.file_mib:
        raise ValueError("choose an encoded image or a custody-only file")
    torch.set_num_threads(1)
    device = str(ctx.device)

    def note(event: str) -> int:
        when = time.time_ns()
        tel.log(
            json.dumps(
                {
                    "handoff": event,
                    "tag": payload.tag,
                    "pid": os.getpid(),
                    "at_ns": when,
                }
            )
        )
        return when

    with torch.inference_mode(), tel.stage("gpu-work"):
        # Repeated dense GEMM exercises the GPU without changing the expected identity result.
        identity = torch.eye(2048, device=device)
        current = identity @ identity
        torch.cuda.synchronize()
        started = note("gpu_started")
        for index in range(payload.iterations):
            current = current @ identity
            if index % 64 == 0:
                ctx.raise_if_cancelled()
        torch.cuda.synchronize()
        if not torch.equal(current, identity):
            raise RuntimeError(
                "the actual GPU GEMM output differs from its identity input"
            )
        asset = None
        file_asset = None
        file_digest = ""
        digest = ""
        if payload.image:
            generator = torch.Generator(device=device).manual_seed(payload.seed)
            pixels = (
                torch.randint(
                    0,
                    256,
                    (payload.size, payload.size, 3),
                    generator=generator,
                    dtype=torch.uint8,
                    device=device,
                )
                .cpu()
                .numpy()
                .tobytes()
            )
            digest = hashlib.sha256(pixels).hexdigest()
            asset = out.save_image(
                ImageFrame(payload.size, payload.size, pixels), format="webp"
            )
        if payload.file_mib:
            # An ordinary bounded file exercises real copy/hash/fsync after encoding ends.
            generator = torch.Generator(device=device).manual_seed(payload.seed)
            block = (
                torch.randint(
                    0,
                    256,
                    (1 << 20,),
                    generator=generator,
                    dtype=torch.uint8,
                    device=device,
                )
                .cpu()
                .numpy()
                .tobytes()
            )
            path = out.temporary_file(".bin")
            hashed = hashlib.sha256()
            with path.open("wb") as writer:
                for index in range(payload.file_mib):
                    ctx.raise_if_cancelled()
                    chunk = index.to_bytes(8, "little") + block[8:]
                    writer.write(chunk)
                    hashed.update(chunk)
            file_digest = hashed.hexdigest()
            file_asset = out.save_file(path)
            path.unlink()
        finished = note("gpu_finished")
    # save_image registered raw pixels; CPU encoding happens in the real output path afterward.
    return Result(
        payload.tag,
        os.getpid(),
        started,
        finished,
        digest,
        [asset] if asset is not None else [],
        [file_asset] if file_asset is not None else [],
        file_digest,
    )
