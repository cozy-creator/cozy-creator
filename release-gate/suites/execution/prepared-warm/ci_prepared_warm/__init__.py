"""A stored linear model with one genuine warm-only native prefix/repeat/suffix program."""

from __future__ import annotations

import json
import os
import time
from collections import OrderedDict
from typing import Annotated, Any

import msgspec
import torch
from cozy_runtime.author import (
    AdjustmentRow,
    AdjustmentRows,
    Adjustments,
    App,
    Context,
    Loader,
    Model,
    ModelArtifact,
    Preflight,
    Telemetry,
    WeightsOutput,
    held,
    uses_components,
)
from cozy_runtime.compute import PreparedRepeat
from cozy_runtime.weights_store import Derivation

app = App()


class Pipeline:
    def __init__(self, config: Any) -> None:
        self.components = {
            "net": torch.nn.Sequential(
                OrderedDict(
                    [
                        (
                            "proj",
                            torch.nn.Linear(
                                config.as_int("in_features"),
                                config.as_int("out_features"),
                                dtype=torch.bfloat16,
                            ),
                        )
                    ]
                )
            )
        }
        self.plan = None
        self.operand = None
        self.output = None
        self.steps = 0
        self.factory_calls = 0
        self.handler_calls = 0
        self.captures = 0
        self.phase_calls = [0, 0, 0]


def build(config: Any) -> Pipeline:
    return Pipeline(config)


class LinearModel(Model[Pipeline]):
    pipe: Pipeline

    def load(self, loader: Loader) -> None:
        self.pipe = loader.construct(Pipeline, factory=build)

    @uses_components("net")
    def reference(self, operand: torch.Tensor) -> torch.Tensor:
        return self.pipe.components["net"](operand).float()


class Make(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 103


@app.entrypoint(accelerator=False, weights=(WeightsOutput("model", 1 << 20),))
def source(payload: Make, ctx: Context) -> ModelArtifact:
    weight = (
        (((torch.arange(16 * 256) + payload.seed) % 31 - 15).float() / 256)
        .reshape(16, 256)
        .bfloat16()
    )
    bias = ((torch.arange(16) - 8).float() / 1024).bfloat16()
    with ctx.output(
        "model",
        Derivation(base=None, order=(("net", "proj.weight"), ("net", "proj.bias"))),
    ) as output:
        output.config("net", {"in_features": 256, "out_features": 16})
        for name, value in (("proj.weight", weight), ("proj.bias", bias)):
            output.put(
                "net",
                name,
                "BF16",
                value.shape,
                value.view(torch.uint8).numpy().tobytes(),
            )
        return ctx.adopt_model(output)


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 103
    steps: Annotated[int, msgspec.Meta(ge=1, le=128)] = 8
    require_prepared: bool = True
    min_free_bytes: Annotated[int, msgspec.Meta(ge=0)] = 0
    hold_ms: Annotated[int, msgspec.Meta(ge=0, le=30000)] = 0


class Facts(msgspec.Struct, frozen=True):
    steps: int
    min_free_bytes: int
    hold_ms: int
    adjustments: tuple[AdjustmentRow, ...]


def resolve(payload: Probe) -> Facts:
    rows = AdjustmentRows()
    steps = held(rows, "steps", payload.steps, 1, 64)
    return Facts(steps, payload.min_free_bytes, payload.hold_ms, tuple(rows.rows))


def prepare(
    ctx: Context,
    facts: Preflight[Facts],
    model: LinearModel,
    tel: Telemetry,
    adj: Adjustments,
) -> bool:
    pipe = model.pipe
    pipe.factory_calls += 1
    adj.extend(facts.adjustments)
    tel.log(
        "prepared-warm factory entered",
        pid=os.getpid(),
        factory_calls=pipe.factory_calls,
    )
    ctx.plan("net")
    until = time.monotonic() + facts.hold_ms / 1000
    while time.monotonic() < until:
        ctx.raise_if_cancelled()
        time.sleep(0.05)
    if pipe.plan is not None and pipe.steps == facts.steps and pipe.plan.valid():
        return True
    if pipe.plan is not None:
        pipe.plan.close()
        pipe.plan = None
    # A real capacity/eligibility decision, using the actual card's current free bytes.
    if torch.cuda.mem_get_info()[0] < facts.min_free_bytes:
        return False
    with torch.inference_mode(False), torch.no_grad():
        pipe.operand = torch.zeros(
            (1, 256), dtype=torch.bfloat16, device=str(ctx.device)
        )
        pipe.output = torch.zeros((1, 16), dtype=torch.float32, device=str(ctx.device))
        net = pipe.components["net"]

        def prefix() -> None:
            pipe.phase_calls[0] += 1
            pipe.output.zero_()

        def repeat(index: Any) -> None:
            del index
            pipe.phase_calls[1] += 1
            pipe.output.add_(net(pipe.operand).float())

        def suffix() -> None:
            pipe.phase_calls[2] += 1
            pipe.output.mul_(0.5)

        pipe.plan = PreparedRepeat.capture(
            repeat,
            prefix=prefix,
            suffix=suffix,
            state=(pipe.operand, pipe.output),
            steps=facts.steps,
            owners=(net,),
            resident_only=False,
        )
    pipe.steps = facts.steps
    pipe.captures += 1
    return True


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True, preflight=resolve, prepare=prepare)
def gpu(
    payload: Probe,
    facts: Preflight[Facts],
    model: LinearModel,
    ctx: Context,
    tel: Telemetry,
    adj: Adjustments,
) -> Report:
    pipe = model.pipe
    pipe.handler_calls += 1
    adj.extend(facts.adjustments)
    valid = pipe.plan is not None and pipe.steps == facts.steps and pipe.plan.valid()
    row = {
        "pid": os.getpid(),
        "request_id": ctx.request_id,
        "seed": payload.seed,
        "factory_calls": pipe.factory_calls,
        "handler_calls": pipe.handler_calls,
        "captures": pipe.captures,
        "prepared": valid,
    }
    if not valid:
        assert not payload.require_prepared, (
            "ordinary inference must never bootstrap capture"
        )
        return Report(True, json.dumps(row, sort_keys=True))
    with torch.inference_mode(False), torch.no_grad():
        value = (
            (((torch.arange(256) + payload.seed) % 17 - 8).float() / 16)
            .reshape(1, 256)
            .bfloat16()
        )
        pipe.operand.copy_(value)
        reference = model.reference(pipe.operand) * (facts.steps * 0.5)
        before = pipe.plan.facts()["host_graph_launches"]
        callbacks = tuple(pipe.phase_calls)
        execution = ctx.plan(pipe.plan)
        _, output = execution.wait()
        assert tuple(pipe.phase_calls) == callbacks, (
            "Python phase callback ran after capture"
        )
        after = pipe.plan.facts()["host_graph_launches"]
        assert after - before == 1
        torch.testing.assert_close(output, reference, rtol=1e-4, atol=1e-5)
        row.update(
            {
                "definition": id(pipe.plan),
                "phase_calls": callbacks,
                "host_graph_launches": after,
                "launches_this_request": after - before,
                "value": output.cpu().tolist(),
                "steps": facts.steps,
            }
        )
    return Report(True, json.dumps(row, sort_keys=True))
