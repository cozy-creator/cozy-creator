"""Actual SVDQ calibration, public checkpoint adoption and held-out encoded consumption."""

from __future__ import annotations

import hashlib
import importlib.metadata
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
    Telemetry,
    UnsupportedInput,
    Weights,
    WeightsOutput,
    uses_components,
)
from cozy_runtime.weights_store import Derivation

app = App()
ROWS, COLS, RANK = 128, 256, 32
ROLE_ORDER = (
    "weight",
    "scale",
    "scale_2",
    "pre_quant_scale",
    "lowrank_down",
    "lowrank_up",
    "input_scale",
)
NATIVE = "cozy.svdq-nvfp4.native-leaf/1"


def source_values(seed: int) -> tuple[torch.Tensor, torch.Tensor]:
    rng = np.random.default_rng(seed)
    weight = torch.from_numpy(
        rng.normal(0, 0.0625, (ROWS, COLS)).astype("<f4")
    ).bfloat16()
    bias = torch.from_numpy(rng.normal(0, 0.0078125, ROWS).astype("<f4")).bfloat16()
    return weight, bias


def operands(seed: int, held_out: bool = False) -> list[torch.Tensor]:
    rng = np.random.default_rng(seed ^ (0xB519 if held_out else 0xCA11))
    gain = np.linspace(0.25, 2.0, COLS, dtype=np.float32)
    return [
        torch.from_numpy(
            (rng.normal(0, 0.125, (16, COLS)) * gain).astype("<f4")
        ).bfloat16()
        for _ in range(2 if held_out else 8)
    ]


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


def build(config: Any) -> Pipeline:
    return Pipeline(config)


class LinearModel(Model[Pipeline], encoded_leaves="accept"):
    pipe: Pipeline

    def load(self, loader: Loader) -> None:
        self.pipe = loader.construct(Pipeline, factory=build)

    @uses_components("net")
    def predict(self, value: torch.Tensor) -> tuple[torch.Tensor, str, str]:
        net = self.pipe.components["net"]
        layer = net.proj
        with torch.inference_mode():
            result = net(value.to(device=layer.bias.device, dtype=torch.bfloat16))
        return (
            result.float().cpu(),
            str(getattr(layer, "provider", "ordinary-linear")),
            str(getattr(layer, "encoding", "plain")),
        )

    @uses_components("net")
    def record(self, batches: list[torch.Tensor]) -> Any:
        from cozy_runtime.derive.svdq import Calibration

        with Calibration(
            torch, self.pipe.components, ("net.proj.weight",)
        ) as calibration:
            for value in batches:
                self.predict(value)
        return calibration


class Request(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 156


class Prediction(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 156
    require_native: bool = False


class Converted(msgspec.Struct):
    passed: bool
    model: ModelArtifact
    detail: str


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=False, weights=(WeightsOutput("model", 1 << 20),))
def source(payload: Request, ctx: Context) -> ModelArtifact:
    weight, bias = source_values(payload.seed)
    with ctx.output(
        "model",
        Derivation(base=None, order=(("net", "proj.weight"), ("net", "proj.bias"))),
    ) as output:
        output.config(
            "net", {"in_features": COLS, "out_features": ROWS, "seed": payload.seed}
        )
        for key, value in (("proj.weight", weight), ("proj.bias", bias)):
            output.put(
                "net",
                key,
                "BF16",
                value.shape,
                value.view(torch.uint8).numpy().tobytes(),
            )
        return ctx.adopt_model(output)


def tensor(raw: bytes, dtype: Any, shape: tuple[int, ...], device: Any = "cpu") -> Any:
    return torch.frombuffer(bytearray(raw), dtype=dtype).reshape(shape).to(device)


def check_residual(
    fitted: Any, values: np.ndarray, stats: Any, device: Any
) -> dict[str, Any]:
    """Reconstruct chosen smoothing directly, never by inverting rounded stored p=1/s."""
    from .oracle import encode_nvfp4

    w = torch.from_numpy(values).to(device)
    xmax = stats.xmax.to(device).float().clamp(min=1e-5)
    wmax = w.abs().amax(0).clamp(min=1e-5)
    alpha = fitted.alpha
    scale = (
        torch.ones_like(xmax)
        if alpha == 0
        else (xmax**alpha / wmax ** (1 - alpha)).clamp(1e-5)
    )
    scale = scale / scale.mean()
    assert (1.0 / scale).cpu().numpy().tobytes() == fitted.companions[2]
    down = tensor(fitted.companions[3], torch.bfloat16, (RANK, COLS), device).float()
    up = tensor(fitted.companions[4], torch.bfloat16, (ROWS, RANK), device).float()
    residual = w * scale - up @ (down * scale)
    reference = encode_nvfp4("proj.weight", residual.cpu().numpy())
    assert fitted.payload == reference.payload
    assert fitted.companions[:2] == tuple(part.raw for part in reference.companions)
    assert fitted.saturated == reference.saturated
    return {
        "alpha": alpha,
        "residual_amax": fitted.amax,
        "residual_max_abs_error": fitted.max_abs_err,
        "residual_reference_sse": reference.sse,
        "residual_reference_energy": reference.energy,
        "residual_reference_sha256": hashlib.sha256(reference.payload).hexdigest(),
        "whole_weight_sse": fitted.sse,
        "whole_weight_energy": fitted.energy,
        "calibration_served_error": fitted.error,
        "calibration_plain_nvfp4_error": fitted.nvfp4_error,
    }


@app.entrypoint(
    accelerator=True,
    weights=(WeightsOutput("svdq", 1 << 20), WeightsOutput("uncalibrated", 1 << 20)),
)
def calibrate(
    payload: Request,
    model: LinearModel,
    weights: Weights[LinearModel],
    ctx: Context,
    tel: Telemetry,
) -> Converted:
    import tensorfs
    from cozy_runtime._build_provenance import COMMIT
    from cozy_runtime.derive import svdq
    from cozy_runtime.derive.quantization import (
        ArtifactQuantizationRequest,
        prepare_source_quantization,
        quantize_component_into,
    )
    from tensorfs import raw

    assert ctx.device.type == "cuda"
    assert callable(raw.quantize_measured)
    checkpoint = ctx.tensorfs_source(weights)
    stored = checkpoint.components["net"].tensors
    original, bias = source_values(payload.seed)
    assert stored["proj.weight"].encoding is None
    source_weight = stored.read("proj.weight")
    assert isinstance(source_weight, tensorfs.Array)
    assert source_weight.tobytes() == original.view(torch.uint8).numpy().tobytes()
    batches = operands(payload.seed)
    calibration = model.record(batches)
    stats = calibration.stats("net", "proj.weight")
    torch.testing.assert_close(
        stats.xmax, torch.cat(batches).float().abs().amax(0), rtol=0, atol=0
    )
    assert stats.rows.shape == (128, COLS), stats.rows.shape
    values = original.float().numpy()
    fitted = svdq.fit(torch, "proj.weight", values, stats, ctx.device)
    detail = check_residual(fitted, values, stats, ctx.device)
    selection = prepare_source_quantization(checkpoint, components=("net",), block=COLS)
    work = Derivation(
        base=checkpoint,
        order=selection.order,
        provenance={
            "gate": "svdq-calibration/1",
            "seed": payload.seed,
            "runtime_commit": COMMIT,
            "tensorfs": tensorfs.__version__,
            "calibration_rows_sha256": hashlib.sha256(
                stats.rows.contiguous().view(torch.uint8).numpy().tobytes()
            ).hexdigest(),
        },
    )
    with ctx.output("uncalibrated", work) as refused:
        try:
            quantize_component_into(
                checkpoint,
                refused,
                ctx,
                ArtifactQuantizationRequest(),
                tel,
                encoding="svdq.nvfp4",
                plan=selection,
                component="net",
                logical_dtype=lambda item: item.logical_dtype,
            )
        except UnsupportedInput as error:
            assert "calibration" in str(error), str(error)
            assert refused.written == 0 and refused.done is None
        else:
            raise AssertionError("SVDQ without real calibration was accepted")
    with ctx.output("svdq", work) as output:
        measured = quantize_component_into(
            checkpoint,
            output,
            ctx,
            ArtifactQuantizationRequest(),
            tel,
            encoding="svdq.nvfp4",
            plan=selection,
            component="net",
            logical_dtype=lambda item: item.logical_dtype,
            calibration=calibration,
        )
        artifact = ctx.adopt_model(output)
    committed = tensorfs.open(artifact.checkpoint, local_files_only=True)
    try:
        item = committed.components["net"].tensors.read("proj.weight")
        assert isinstance(item, tensorfs.Encoded)
        assert item.scheme == "svdq.nvfp4" and item.shape == (ROWS, COLS)
        assert item.dtype == "BF16"
        geometry = {
            "weight": ("F4", (ROWS, COLS)),
            "scale": ("F8_E4M3", (ROWS, COLS // 16)),
            "scale_2": ("F32", ()),
            "pre_quant_scale": ("F32", (COLS,)),
            "lowrank_down": ("BF16", (RANK, COLS)),
            "lowrank_up": ("BF16", (ROWS, RANK)),
            "input_scale": ("F32", ()),
        }
        expected = {
            "weight": fitted.payload,
            **dict(zip(ROLE_ORDER[1:], fitted.companions, strict=True)),
        }
        assert set(item.parts) == set(expected)
        for role, reference in expected.items():
            assert (item.parts[role].dtype, item.parts[role].shape) == geometry[role], (
                role
            )
            assert item.parts[role].tobytes() == reference, role
        assert measured.encoded_keys == 1 and measured.reused_keys == 0
        assert measured.source_bytes_read == ROWS * COLS * 2
        assert measured.new_bytes_written == sum(
            len(value) for value in expected.values()
        )
        inherited_bias = committed.components["net"].tensors.read("proj.bias")
        assert isinstance(inherited_bias, tensorfs.Array)
        assert inherited_bias.tobytes() == bias.view(torch.uint8).numpy().tobytes()
        assert committed.verify().ok
        detail.update(
            seed=payload.seed,
            checkpoint=committed.id,
            runtime_commit=COMMIT,
            runtime=importlib.metadata.version("cozy-runtime"),
            tensorfs=tensorfs.__version__,
            torch=str(torch.__version__),
            device=torch.cuda.get_device_name(),
            calibration_rows=stats.rows.shape[0],
            calibration_channels=stats.xmax.numel(),
            measured=msgspec.to_builtins(measured),
            missing_calibration_refused=True,
            stored_parts={
                role: {
                    "dtype": part.dtype,
                    "shape": list(part.shape),
                    "sha256": hashlib.sha256(part.tobytes()).hexdigest(),
                }
                for role, part in item.parts.items()
            },
        )
    finally:
        committed.close()
    return Converted(True, artifact, json.dumps(detail, sort_keys=True))


def decoded_weight(parts: dict[str, Any]) -> torch.Tensor:
    """Independent exact stored-value law, not the production TensorFS/SVDQ decoder."""
    from .oracle import E2M1_DECODE32, E4M3_MAG32

    packed = np.frombuffer(parts["weight"].tobytes(), dtype=np.uint8).reshape(
        ROWS, COLS // 2
    )
    codes = np.empty((ROWS, COLS), dtype=np.uint8)
    codes[:, 0::2], codes[:, 1::2] = packed & 15, packed >> 4
    scale = np.frombuffer(parts["scale"].tobytes(), dtype=np.uint8).reshape(
        ROWS, COLS // 16
    )
    second = np.frombuffer(parts["scale_2"].tobytes(), dtype="<f4").item()
    residual = (
        E2M1_DECODE32[codes].astype(np.float64)
        * np.repeat(E4M3_MAG32[scale].astype(np.float64), 16, axis=1)
        * second
    )
    pre = tensor(parts["pre_quant_scale"].tobytes(), torch.float32, (COLS,)).double()
    down = tensor(
        parts["lowrank_down"].tobytes(), torch.bfloat16, (RANK, COLS)
    ).double()
    up = tensor(parts["lowrank_up"].tobytes(), torch.bfloat16, (ROWS, RANK)).double()
    return torch.from_numpy(residual) * pre + up @ down


@app.entrypoint(accelerator=True)
def predict(
    payload: Prediction,
    model: LinearModel,
    weights: Weights[LinearModel],
    ctx: Context,
) -> Report:
    import tensorfs

    assert ctx.device.type == "cuda"
    checkpoint = ctx.tensorfs_source(weights)
    held = checkpoint.components["net"].tensors
    encoded = held.read("proj.weight")
    assert isinstance(encoded, tensorfs.Encoded)
    assert encoded.scheme == "svdq.nvfp4" and encoded.shape == (ROWS, COLS)
    reference = decoded_weight(encoded.parts)
    original, original_bias = source_values(payload.seed)
    inherited_bias = held.read("proj.bias")
    assert isinstance(inherited_bias, tensorfs.Array)
    bias = tensor(inherited_bias.tobytes(), torch.bfloat16, (ROWS,))
    assert torch.equal(bias, original_bias)
    rows = []
    for value in operands(payload.seed, held_out=True):
        actual, provider, scheme = model.predict(value)
        native = provider == NATIVE
        if payload.require_native:
            assert native and scheme == "svdq.nvfp4", (provider, scheme)
        else:
            assert native or (provider == "ordinary-linear" and scheme == "plain"), (
                provider,
                scheme,
            )
        expected = torch.nn.functional.linear(value.double(), reference, bias.double())
        original_output = torch.nn.functional.linear(
            value.double(), original.double(), bias.double()
        )
        error = actual.double() - expected
        relative = float(
            torch.linalg.vector_norm(error) / torch.linalg.vector_norm(expected)
        )
        original_relative = float(
            torch.linalg.vector_norm(actual.double() - original_output)
            / torch.linalg.vector_norm(original_output)
        )
        assert torch.isfinite(actual).all()
        # Explicit fixture tolerance, not a claim about every SVDQ model or distribution.
        assert relative <= (0.10 if native else 0.02), (relative, provider)
        assert original_relative <= 0.20, original_relative
        rows.append(
            {
                "provider": provider,
                "module_encoding": scheme,
                "consumption": "native-encoded-gemm"
                if native
                else "decoded-weight-gemm",
                "heldout_relative_to_stored": relative,
                "heldout_relative_to_original": original_relative,
                "heldout_max_abs_error": float(error.abs().max()),
                "output_sha256": hashlib.sha256(actual.numpy().tobytes()).hexdigest(),
            }
        )
    assert checkpoint.verify().ok
    return Report(
        True,
        json.dumps(
            {
                "seed": payload.seed,
                "checkpoint": checkpoint.id,
                "device": torch.cuda.get_device_name(),
                "cases": rows,
            },
            sort_keys=True,
        ),
    )
