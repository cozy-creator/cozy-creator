"""Real CUDA computation and a raw image consumed by the ordinary frame codec.

Both workload modes use the same entrypoint/construction so a healthy executor is reusable.
The cell does not load a checkpoint or claim model-quality/performance qualification.
"""

import hashlib
import json
import os
import struct
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
    ModelArtifact,
    Outputs,
    Telemetry,
    WeightsOutput,
    invocable,
)
from cozy_runtime.weights_store import Derivation

app = App()


class LoopRequest(msgspec.Struct, forbid_unknown_fields=True):
    steps: Annotated[int, msgspec.Meta(ge=1, le=100000)] = 30000


class LoopResult(msgspec.Struct):
    passed: bool
    steps: int
    body_python_calls: int
    host_graph_launches: int
    execution_mode: str
    value: float


@app.entrypoint(accelerator=True)
def native_loop(payload: LoopRequest, ctx: Context) -> LoopResult:
    import torch
    from cozy_runtime.compute import PreparedRepeat

    if ctx.device.type != "cuda":
        raise ValueError("the installed native loop requires CUDA")
    state = torch.zeros(1, dtype=torch.float32, device=str(ctx.device))
    calls = 0

    def step(_index):
        nonlocal calls
        calls += 1
        state.add_(1)

    plan = PreparedRepeat.capture(step, state=(state,), steps=payload.steps)
    try:
        execution = ctx.plan(plan)
        assert execution is not None
        time.sleep(0.25)  # The GPU must continue while this application thread is parked.
        value = float(execution.wait()[0].item())
        facts = plan.facts()
        if value != payload.steps or calls != 2 or facts["host_graph_launches"] != 1:
            raise RuntimeError("installed native execution differs from its declared loop")
        if facts["execution_mode"] != "cuda_conditional_while":
            raise RuntimeError("the installed path did not use a GPU-driven loop")
        return LoopResult(True, payload.steps, calls, 1, facts["execution_mode"], value)
    finally:
        plan.close()


class FamilyRequest(msgspec.Struct, forbid_unknown_fields=True):
    value: Annotated[int, msgspec.Meta(ge=1, le=64)] = 7


class ChildResult(msgspec.Struct):
    value: int
    pid: int


class FamilyResult(msgspec.Struct):
    passed: bool
    parent_pid: int
    child_pid: int


@app.entrypoint(internal=True, accelerator=True)
@invocable
async def gpu_child(ctx: Context, *, payload: FamilyRequest) -> ChildResult:
    import torch

    if ctx.device.type != "cuda":
        raise ValueError("the family proof child requires CUDA")
    value = torch.tensor(payload.value, dtype=torch.int64, device=str(ctx.device))
    return ChildResult(int((value + 1).item()), os.getpid())


@app.entrypoint(accelerator=True)
@invocable
async def gpu_parent(ctx: Context, *, payload: FamilyRequest) -> FamilyResult:
    import torch

    if ctx.device.type != "cuda":
        raise ValueError("the family proof parent requires CUDA")
    held = torch.full((32,), payload.value, dtype=torch.int64, device=str(ctx.device))
    child = await gpu_child(ctx, payload=payload)
    if child.value != payload.value + 1 or int(held.sum().item()) != 32 * payload.value:
        raise RuntimeError("parent/child CUDA values differ from their expected results")
    if child.pid == os.getpid():
        raise RuntimeError("an inline child did not exercise separate-executor GPU admission")
    return FamilyResult(True, os.getpid(), child.pid)


class DeriveRequest(msgspec.Struct, forbid_unknown_fields=True):
    seed: int
    recipe: str = "v1"


class DeriveResult(msgspec.Struct):
    artifact: ModelArtifact
    repo_id: str
    lane: str
    tree: str
    reused: bool
    written_bytes: int
    exact_bytes: bool


@app.entrypoint(accelerator=False, weights=(WeightsOutput("prepared", 4096),))
@invocable
async def derive(ctx: Context, *, payload: DeriveRequest) -> DeriveResult:
    """Real public derivation/adoption, including persistent lookup on a later invocation."""
    work = Derivation(
        base=None,
        order=(("probe", "values"),),
        provenance={"recipe": payload.recipe, "seed": payload.seed},
    )
    output = ctx.output("prepared", work)
    reused = output.done is not None
    data = struct.pack("<4f", *(float((payload.seed + i) % 100) for i in range(4)))
    if not output.has("probe", "values"):
        output.put("probe", "values", "F32", (4,), data)
    checkpoint = output.commit()
    actual = bytearray(len(data))
    checkpoint.components["probe"].tensors.read_into("values", actual)
    if bytes(actual) != data:
        raise RuntimeError(
            "the committed TensorFS values differ from the prepared bytes"
        )
    artifact = ctx.adopt_model(output)
    expected_repo = "local/derived-" + artifact.transaction.removeprefix("sha256:")
    if checkpoint.repo_id != expected_repo or checkpoint.lane != "prepared":
        raise RuntimeError(
            "the replay was not retained in its own repository and output lane"
        )
    return DeriveResult(
        artifact,
        checkpoint.repo_id,
        checkpoint.lane,
        checkpoint.tree or "",
        reused,
        output.written,
        True,
    )


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
