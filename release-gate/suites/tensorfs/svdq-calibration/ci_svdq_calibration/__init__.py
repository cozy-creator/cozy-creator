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
    mode: str = "basic"


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
    from cozy_runtime.derive import native_quantization
    from cozy_runtime.derive.quantization import (
        ArtifactQuantizationRequest,
        prepare_source_quantization,
        quantize_component_into,
    )

    from .historical_svdq import fit as historical_fit
    from .native import WORKSPACE, check_selected

    assert ctx.device.type == "cuda"
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
    # Frozen historical arithmetic is only a comparison; it cannot feed the native output.
    old = historical_fit(torch, "proj.weight", values, stats, str(ctx.device))
    selection = prepare_source_quantization(checkpoint, components=("net",), block=COLS)
    resolved = native_quantization.resolved(
        checkpoint, selection, "svdq.nvfp4", lambda item: item.logical_dtype
    )
    with native_quantization.snapshot(
        checkpoint, selection, "svdq.nvfp4", calibration, ctx
    ) as snapshot:
        assert snapshot is not None
        work = Derivation(
            base=checkpoint,
            sources=(snapshot,),
            order=selection.order,
            provenance={
                "gate": "svdq-calibration/native-faer-1",
                "seed": payload.seed,
                "runtime_commit": COMMIT,
                "tensorfs": tensorfs.__version__,
                **native_quantization.provenance(checkpoint, resolved, snapshot),
            },
        )
        with ctx.output("uncalibrated", work) as refused:
            try:
                quantize_component_into(
                    checkpoint,
                    refused,
                    ctx,
                    ArtifactQuantizationRequest(fit_workspace_bytes=WORKSPACE),
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
                ArtifactQuantizationRequest(fit_workspace_bytes=WORKSPACE),
                tel,
                encoding="svdq.nvfp4",
                plan=selection,
                component="net",
                logical_dtype=lambda item: item.logical_dtype,
                calibration=calibration,
            )
            artifact = ctx.adopt_model(output)
        calibration_id = snapshot.id
    committed = tensorfs.open(artifact.checkpoint, local_files_only=True)
    try:
        item = committed.components["net"].tensors.read("proj.weight")
        assert isinstance(item, tensorfs.Encoded)
        assert (
            item.scheme == "svdq.nvfp4"
            and item.shape == (ROWS, COLS)
            and item.dtype == "BF16"
        )
        assert measured.encoded_keys == 1 and measured.reused_keys == 0
        assert measured.source_bytes_read == ROWS * COLS * 2
        assert measured.new_bytes_written == sum(
            len(part.tobytes()) for part in item.parts.values()
        )
        (row,) = measured.tensors
        detail = check_selected(
            item, values, row, calibration=stats.rows.float().numpy()
        )
        fit = row["fit"]
        assert fit["calibration_bytes_read"] == COLS * 4 + 128 * COLS * 2
        assert fit["calibration_error"] <= old.error * 1.25 + 0.02, (fit, old.error)
        assert fit["calibration_error"] < fit["plain_nvfp4_error"], fit
        inherited_bias = committed.components["net"].tensors.read("proj.bias")
        assert isinstance(inherited_bias, tensorfs.Array)
        assert inherited_bias.tobytes() == bias.view(torch.uint8).numpy().tobytes()
        assert committed.verify().ok
        detail.update(
            seed=payload.seed,
            checkpoint=committed.id,
            calibration_checkpoint=calibration_id,
            runtime_commit=COMMIT,
            runtime=importlib.metadata.version("cozy-runtime"),
            tensorfs=tensorfs.__version__,
            implementation=tensorfs.raw.quantization_implementation(),
            torch=str(torch.__version__),
            device=torch.cuda.get_device_name(),
            calibration_rows=stats.rows.shape[0],
            calibration_channels=stats.xmax.numel(),
            measured=msgspec.to_builtins(measured),
            missing_calibration_refused=True,
            historical_reference={
                "producer": "torch-svdq-frozen/1",
                "alpha": old.alpha,
                "calibration_error": old.error,
                "plain_nvfp4_error": old.nvfp4_error,
                "whole_weight_sse": old.sse,
                "whole_weight_energy": old.energy,
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


# Register the real CPU Writer proof in this same ordinary release-gate package.
from . import native as native
