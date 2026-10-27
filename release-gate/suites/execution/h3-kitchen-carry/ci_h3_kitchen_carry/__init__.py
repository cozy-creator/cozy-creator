"""H3 Kitchen-producer summaries under native resident segments, at production width."""

from __future__ import annotations

import json
from typing import Annotated, Any

import msgspec
import torch
from cozy_runtime.author import (
    App,
    Context,
    Loader,
    Model,
    ModelArtifact,
    Telemetry,
    WeightsOutput,
    uses_components,
)
from cozy_runtime.author.kernels import Plan
from cozy_runtime.weights_store import Derivation

from . import exercise
from .candidate import glue, producer

app = App()
DTYPES = {torch.float32: "F32", torch.bfloat16: "BF16"}


class Pipeline:
    def __init__(self, config: Any) -> None:
        # A one-component model's config is that component's own (`output.config`), not a section.
        self.components = {"transformer": exercise.build(config.mapping())}


class GateModel(
    Model[Pipeline],
    encoded_leaves="accept",
    # H3Model's: the DiT's attention holds at 4 bits.
    attention_bits={"transformer": 4},
):
    pipe: Pipeline

    def load(self, loader: Loader) -> None:
        self.pipe = loader.construct(Pipeline, factory=Pipeline)

    def execution_plan(self, plan: Plan) -> str:
        """H3Model.execution_plan's DiT half: the rotary hook and the fused glue."""
        producer.retain_rotary(self.pipe.components["transformer"])
        return glue.install(self, self.pipe, plan)

    @uses_components("transformer")
    def check(self, payload: Probe, ctx: Context, tel: Telemetry) -> dict[str, Any]:
        transformer = self.pipe.components["transformer"]
        if payload.mode == "warm":
            return exercise.warm(transformer, payload, ctx)
        return exercise.gate(transformer, payload, ctx, tel)

    @uses_components("transformer")
    def cpu_check(self, payload: Probe) -> dict[str, Any]:
        return exercise.cpu(self.pipe.components["transformer"], payload)


class Make(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 2610271
    # The gate needs >= 3 (blocks 0-1 are dense paths); the CPU path loads any.
    layers: Annotated[int, msgspec.Meta(ge=1, le=8)] = 4
    refiner_layers: Annotated[int, msgspec.Meta(ge=0, le=2)] = 1


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    mode: str = "gate"
    seed: int = 2610272
    latent_frames: Annotated[int, msgspec.Meta(ge=2, le=107)] = 107
    text_tokens: Annotated[int, msgspec.Meta(ge=4, le=1024)] = 256
    cases: str = "R15,S1,S2,S4"


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=False, weights=(WeightsOutput("model", 16 << 30),))
def source(payload: Make, ctx: Context) -> ModelArtifact:
    """The gate's checkpoint, written on the machine that runs it."""
    config, state = exercise.checkpoint(payload.seed, payload.layers, payload.refiner_layers)
    order = tuple(("transformer", key) for key in state)
    with ctx.output("model", Derivation(base=None, order=order)) as output:
        output.config("transformer", config)
        for key, value in state.items():
            dtype = DTYPES[value.dtype]
            output.put("transformer", key, dtype, value.shape, value.contiguous().view(torch.uint8).numpy())
        return ctx.adopt_model(output)


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, model: GateModel, ctx: Context, tel: Telemetry) -> Report:
    if payload.mode not in ("warm", "gate"):
        raise ValueError("mode is warm or gate")
    report = model.check(payload, ctx, tel)
    tel.log("h3 kitchen carry", mode=payload.mode, passed=report.get("passed"))
    return Report(bool(report.get("passed", payload.mode == "warm")), json.dumps(report, default=str))


@app.entrypoint(accelerator=False)
def cpu(payload: Probe, model: GateModel, ctx: Context, tel: Telemetry) -> Report:
    """The model's load, fit, plan and selection on a CPU executor, up to the CUDA-only steps."""
    report = model.cpu_check(payload)
    tel.log("h3 kitchen carry cpu", passed=report["passed"])
    return Report(report["passed"], json.dumps(report, default=str))
