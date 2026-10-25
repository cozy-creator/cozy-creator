"""An actual authored linear model and recipes, exercised only through ordinary Cozy APIs."""

from __future__ import annotations

import hashlib
import json
from collections import OrderedDict
from typing import Any

import msgspec
import numpy as np
import torch
from cozy_runtime.author import (
    App,
    Context,
    Loader,
    Model,
    ModelArtifact,
    QuantizationPlan,
    Recipe,
    Weights,
    WeightsOutput,
    uses_components,
)
from cozy_runtime.weights_store import Derivation

app = App()


def source_values(seed: int) -> tuple[torch.Tensor, torch.Tensor]:
    generator = np.random.default_rng(seed)
    weight = torch.from_numpy(
        generator.normal(0, 0.0625, (16, 256)).astype(np.float32)
    ).bfloat16()
    bias = torch.from_numpy(
        generator.normal(0, 0.0078125, 16).astype(np.float32)
    ).bfloat16()
    return weight, bias


class Pipeline:
    def __init__(self, config: Any) -> None:
        settings = config.mapping()["net"]
        self.components = {
            "net": torch.nn.Sequential(
                OrderedDict(
                    [
                        (
                            "proj",
                            torch.nn.Linear(
                                int(settings["in_features"]),
                                int(settings["out_features"]),
                                dtype=torch.bfloat16,
                            ),
                        )
                    ]
                )
            )
        }


def build(config: Any) -> Pipeline:
    return Pipeline(config)


class LinearModel(Model[Pipeline], encoded_leaves="accept"):
    pipe: Pipeline

    def load(self, loader: Loader) -> None:
        self.pipe = loader.construct(Pipeline, factory=build)

    @uses_components("net")
    def predict(self, operand: torch.Tensor) -> tuple[torch.Tensor, str, str]:
        net = self.pipe.components["net"]
        layer = net.proj
        with torch.inference_mode():
            result = net(operand.to(device=layer.bias.device, dtype=torch.bfloat16))
        return (
            result.float().cpu(),
            str(getattr(layer, "provider", "ordinary-linear")),
            str(getattr(layer, "encoding", "plain")),
        )


INT8 = Recipe(
    lane="int8",
    model=LinearModel,
    plan=QuantizationPlan(
        encoding="int8.convrot", components=("net",), output_precision="preserve"
    ),
)
NVFP4 = Recipe(
    lane="nvfp4",
    model=LinearModel,
    plan=QuantizationPlan(
        encoding="nvfp4", components=("net",), output_precision="preserve"
    ),
)


class Make(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 103


@app.entrypoint(accelerator=False, weights=(WeightsOutput("model", 1 << 20),))
def source(payload: Make, ctx: Context) -> ModelArtifact:
    weight, bias = source_values(payload.seed)
    with ctx.output(
        "model",
        Derivation(base=None, order=(("net", "proj.weight"), ("net", "proj.bias"))),
    ) as output:
        output.config("net", {"in_features": 256, "out_features": 16})
        output.put(
            "net",
            "proj.weight",
            "BF16",
            weight.shape,
            weight.view(torch.uint8).numpy().tobytes(),
        )
        output.put(
            "net",
            "proj.bias",
            "BF16",
            bias.shape,
            bias.view(torch.uint8).numpy().tobytes(),
        )
        return ctx.adopt_model(output)


class Predict(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 103
    encoding: str = "int8.convrot"


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def predict(
    payload: Predict, model: LinearModel, weights: Weights[LinearModel], ctx: Context
) -> Report:
    from tensorfs import raw

    assert payload.encoding in ("int8.convrot", "nvfp4")
    checkpoint = ctx.tensorfs_source(weights)
    held = checkpoint.components["net"].tensors
    encoded = held.read("proj.weight")
    assert encoded.scheme == payload.encoding, encoded.scheme
    parts = {
        name: (part.dtype, part.shape, part.tobytes())
        for name, part in encoded.parts.items()
    }
    decoded = np.empty((16, 256), dtype=np.float32)
    raw.decode_into(
        encoded.scheme,
        parts,
        decoded,
        dtype="F32",
        shape=decoded.shape,
        params=encoded.params,
    )
    original, original_bias = source_values(payload.seed)
    bias = torch.frombuffer(
        bytearray(held.read("proj.bias").tobytes()), dtype=torch.bfloat16
    ).float()
    assert torch.equal(bias, original_bias.float())
    reference_weight = torch.from_numpy(decoded)
    relative_weight = float(
        torch.linalg.vector_norm(reference_weight - original.float())
        / torch.linalg.vector_norm(original.float())
    )
    assert relative_weight <= (0.025 if payload.encoding == "int8.convrot" else 0.15), (
        relative_weight
    )
    generator = np.random.default_rng(payload.seed ^ 0x5A17)
    operand = torch.from_numpy(
        generator.normal(0, 0.125, (4, 256)).astype(np.float32)
    ).bfloat16()
    actual, provider, consumed_encoding = model.predict(operand)
    assert consumed_encoding == payload.encoding, (provider, consumed_encoding)
    expected = torch.nn.functional.linear(operand.float(), reference_weight, bias)
    relative_output = float(
        torch.linalg.vector_norm(actual - expected) / torch.linalg.vector_norm(expected)
    )
    max_error = float((actual - expected).abs().max())
    assert bool(torch.isfinite(actual).all())
    assert relative_output <= 0.035 and max_error <= 0.025, (relative_output, max_error)
    assert checkpoint.verify().ok
    return Report(
        True,
        json.dumps(
            {
                "seed": payload.seed,
                "encoding": payload.encoding,
                "checkpoint": checkpoint.id,
                "provider": provider,
                "consumed_encoding": consumed_encoding,
                "decoded_weight_relative_error": relative_weight,
                "inference_relative_error": relative_output,
                "inference_max_abs_error": max_error,
                "output_shape": list(actual.shape),
                "output_sha256": hashlib.sha256(actual.numpy().tobytes()).hexdigest(),
                "weight_payload_sha256": hashlib.sha256(parts["weight"][2]).hexdigest(),
            },
            sort_keys=True,
        ),
    )
