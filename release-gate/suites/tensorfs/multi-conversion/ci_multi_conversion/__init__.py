"""The tensorfs suite's multi-conversion cell: a model of two linear stacks whose int8 recipe opens one output
and encodes each component in its own `quantize_component_into` call, each with its own plan, as minimax-h3-tools'
`_quantize_lane` does. Every first-party declarative `QuantizationPlan` encodes in one conversion, so none of them
reaches this path; H3 does, but costs ~66 GB per source.

`source` writes the bf16 weights, `INT8` writes the lane, `inspect` reads the lane's commit back and checks that every
component's weights are encoded and decode near the source, and that its biases are the source's bytes."""

from operator import attrgetter
from typing import Any

import msgspec
import numpy as np
from cozy_runtime.author import (
    App,
    Context,
    DerivationHook,
    Model,
    ModelArtifact,
    Recipe,
    Telemetry,
    Weights,
    WeightsOutput,
)
from cozy_runtime.derive.quantization import (
    ArtifactQuantizationRequest,
    column_block,
    prepare_source_quantization,
    quantize_component_into,
)
from cozy_runtime.weights_store import Derivation

app = App()
ENCODING = "int8.convrot"
#: ConvRot int8 of these weights is ~0.007 (recipe-quantize's record); 0.025 is that fixture's bar.
MAX_RELATIVE_ERROR = 0.025
#: Per component, each linear layer's (out, in).
LAYERS = {"encoder": ((256, 256), (256, 256)), "decoder": ((256, 256), (64, 256))}


class TwoStacks(Model[Any]):
    """Two linear stacks, `encoder` and `decoder`: the checkpoint layout `INT8` reads. Nothing serves it."""


def bf16(values: np.ndarray) -> np.ndarray:
    """Round-to-nearest-even float32 → bfloat16 bits."""
    bits = values.astype(np.float32).view(np.uint32)
    return ((bits + 0x7FFF + ((bits >> 16) & 1)) >> 16).astype(np.uint16)


def f32(raw: bytes, shape: tuple[int, ...]) -> np.ndarray:
    return (np.frombuffer(raw, np.uint16).astype(np.uint32) << 16).view(np.float32).reshape(shape)


def tensors(seed: int) -> dict[tuple[str, str], np.ndarray]:
    """The source's tensors in construction order, as bfloat16 bits."""
    out = {}
    for c, (component, layers) in enumerate(LAYERS.items()):
        for i, (rows, cols) in enumerate(layers):
            rng = np.random.default_rng([seed, c, i])
            out[component, f"l{i}.weight"] = bf16(rng.normal(0, 0.0625, (rows, cols)))
            out[component, f"l{i}.bias"] = bf16(rng.normal(0, 0.0078125, rows))
    return out


class Make(msgspec.Struct, forbid_unknown_fields=True):
    seed: int


@app.entrypoint(accelerator=False, weights=(WeightsOutput("model", 1 << 20),))
def source(payload: Make, ctx: Context) -> ModelArtifact:
    held = tensors(payload.seed)
    with ctx.output("model", Derivation(base=None, order=tuple(held))) as output:
        for component, layers in LAYERS.items():
            output.config(component, {"layers": [list(layer) for layer in layers]})
        for (component, key), bits in held.items():
            output.put(component, key, "BF16", bits.shape, bits.tobytes())
        return ctx.adopt_model(output)


def quantize(ctx: Context, tel: Telemetry, source: Weights[Any], output: str) -> ModelArtifact:
    checkpoint = ctx.tensorfs_source(source)
    request = ArtifactQuantizationRequest()
    plans = [
        prepare_source_quantization(checkpoint, components=(component,), block=column_block(ENCODING))
        for component in LAYERS
    ]
    with ctx.output(output, Derivation(base=checkpoint, order=plans[0].order)) as out:
        for plan in plans:
            (component,) = plan.components
            with tel.stage(f"{output}-{component}"):
                stats = quantize_component_into(
                    checkpoint, out, ctx, request, tel, encoding=ENCODING, plan=plan,
                    component=component, logical_dtype=attrgetter("logical_dtype"),
                )  # fmt: skip
            tel.log("encoded component", component=component, encoded_keys=stats.encoded_keys)
        return ctx.adopt_model(out)


INT8 = Recipe(lane="int8", model=TwoStacks, plan=DerivationHook(quantize))


class Inspect(msgspec.Struct, forbid_unknown_fields=True):
    seed: int


class Held(msgspec.Struct):
    component: str
    key: str
    scheme: str | None
    relative_error: float | None


class Report(msgspec.Struct):
    checkpoint: str
    tensors: list[Held]


@app.entrypoint(accelerator=False)
def inspect(payload: Inspect, model: Weights[TwoStacks], ctx: Context) -> Report:
    checkpoint = ctx.tensorfs_source(model)
    held, problems = [], []
    for (component, key), bits in tensors(payload.seed).items():
        stored = checkpoint.components[component].tensors if component in checkpoint.components else {}
        if key not in stored:
            problems.append(f"{component}/{key} is missing")
            continue
        encoding = stored[key].encoding
        scheme, error = encoding and encoding.scheme, None
        if key.endswith(".weight"):
            want = f32(bits.tobytes(), bits.shape)
            got = f32(stored.read(key, decode=True).tobytes(), bits.shape)
            error = float(np.linalg.norm(got - want) / np.linalg.norm(want))
            if scheme != ENCODING:
                problems.append(f"{component}/{key} is {scheme or 'plain'}, not {ENCODING}")
            elif error > MAX_RELATIVE_ERROR:
                problems.append(f"{component}/{key} decodes {error:.4f} from the source (bar {MAX_RELATIVE_ERROR})")
        elif scheme is not None or stored.read(key).tobytes() != bits.tobytes():
            problems.append(f"{component}/{key} is not the source's bytes")
        held.append(Held(component, key, scheme, error))
    if problems:
        raise ValueError(f"{checkpoint.id}: " + "; ".join(problems))
    return Report(checkpoint.id, held)
