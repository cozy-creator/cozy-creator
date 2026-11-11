"""Actual SVDQ calibration, public checkpoint adoption and held-out encoded consumption, on the Runtime's public API.
The Runtime's Output reuse and resume are its built-in `bench.quantize_reuse`; TensorFS's own fitting is the program
`programs/svdq_native.py` (xs-174)."""

from __future__ import annotations

import hashlib
import importlib.metadata
import json
import resource
import time
from collections import OrderedDict
from typing import TYPE_CHECKING, cast

import cozy_runtime
import msgspec
import numpy as np
import torch

# The pinned typecheck Runtime (0.45.0) predates the bench: drop the ignore when the pin moves past it.
from cozy_runtime import bench  # type: ignore[attr-defined]
from cozy_runtime.author import (
    App,
    Config,
    Context,
    Loader,
    Model,
    ModelArtifact,
    Telemetry,
    UnsupportedInput,
    Weights,
    WeightsOutput,
    call,
    uses_components,
)
from cozy_runtime.weights_store import Derivation

if TYPE_CHECKING:
    from cozy_runtime.derive.svdq import Calibration

    from ._types import ArrayTensor, Converted as Row, EncodedTensor, Parts

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
    def __init__(self, config: Config) -> None:
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


def build(config: Config) -> Pipeline:
    return Pipeline(config)


class LinearModel(Model[Pipeline]):
    pipe: Pipeline

    def load(self, loader: Loader) -> None:
        self.pipe = loader.construct(build)

    @uses_components("net")
    def predict(self, value: torch.Tensor) -> tuple[torch.Tensor, str, str]:
        net = self.pipe.components["net"]
        layer = cast(torch.nn.Linear, net.proj)  # cast: the Linear the Pipeline built
        with torch.inference_mode():
            result = net(value.to(device=layer.bias.device, dtype=torch.bfloat16))
        return (
            result.float().cpu(),
            str(getattr(layer, "provider", "ordinary-linear")),
            str(getattr(layer, "encoding", "plain")),
        )

    @uses_components("net")
    def record(self, batches: list[torch.Tensor]) -> Calibration:
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
    #: calibrate's `conversion` (JSON): the fit's row and the bytes it wrote, checked against the stored parts
    conversion: str = ""


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


def tensor(raw: bytes, dtype: torch.dtype, shape: tuple[int, ...], device: str | torch.device = "cpu") -> torch.Tensor:
    return torch.frombuffer(bytearray(raw), dtype=dtype).reshape(shape).to(device)


def plain(item: object) -> ArrayTensor:
    """A stored tensor with no encoding: its bytes, no scheme or parts."""
    assert callable(getattr(item, "tobytes", None)) and not hasattr(item, "parts"), type(item).__name__
    return cast("ArrayTensor", item)  # cast: checked above


def encoded(item: object) -> EncodedTensor:
    """A stored tensor with an encoding: its scheme and parts."""
    assert hasattr(item, "scheme") and hasattr(item, "parts"), type(item).__name__
    return cast("EncodedTensor", item)  # cast: checked above


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
    from cozy_runtime.derive import native_quantization
    from cozy_runtime.derive.quantization import (
        ArtifactQuantizationRequest,
        prepare_source_quantization,
        quantize_component_into,
    )

    from .checks import WORKSPACE
    from .historical_svdq import fit as historical_fit

    assert ctx.device.type == "cuda"
    checkpoint = ctx.tensorfs_source(weights)
    stored = checkpoint.components["net"].tensors
    original = source_values(payload.seed)[0]
    assert stored["proj.weight"].encoding is None
    source_weight = plain(stored.read("proj.weight"))
    assert source_weight.tobytes() == original.view(torch.uint8).numpy().tobytes()
    batches = operands(payload.seed)
    calibration_began = time.perf_counter()
    calibration = model.record(batches)
    calibration_seconds = time.perf_counter() - calibration_began
    stats = calibration.stats("net", "proj.weight")
    torch.testing.assert_close(
        stats.xmax, torch.cat(batches).float().abs().amax(0), rtol=0, atol=0
    )
    assert stats.rows.shape == (128, COLS), stats.rows.shape
    values = original.float().numpy()
    # Frozen historical arithmetic is only a comparison; it cannot feed the native output.
    reference_began = time.perf_counter()
    old = historical_fit(torch, "proj.weight", values, stats, str(ctx.device))
    historical_fit_seconds = time.perf_counter() - reference_began
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
                "runtime": cozy_runtime.__version__,
                "tensorfs": importlib.metadata.version("tensorfs"),
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
            conversion_began = time.perf_counter()
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
            conversion_seconds = time.perf_counter() - conversion_began
            adoption_began = time.perf_counter()
            artifact = ctx.adopt_model(output)
            adoption_seconds = time.perf_counter() - adoption_began
        calibration_id = snapshot.id
    # The stored parts are predict's to check, through its own grant of this artifact (ctx.tensorfs_source).
    assert measured.encoded_keys == 1 and measured.reused_keys == 0
    assert measured.source_bytes_read == ROWS * COLS * 2
    (row,) = measured.tensors
    fit = row["fit"]
    assert fit["calibration_bytes_read"] == COLS * 4 + 128 * COLS * 2
    assert fit["calibration_error"] <= old.error * 1.25 + 0.02, (fit, old.error)
    assert fit["calibration_error"] < fit["plain_nvfp4_error"], fit
    detail = {
        "seed": payload.seed,
        "conversion": json.dumps({"row": row, "new_bytes_written": measured.new_bytes_written}, sort_keys=True),
        "calibration_checkpoint": calibration_id,
        "runtime": cozy_runtime.__version__,
        "tensorfs": importlib.metadata.version("tensorfs"),
        "torch": str(torch.__version__),
        "device": torch.cuda.get_device_name(),
        "calibration_rows": stats.rows.shape[0],
        "calibration_channels": stats.xmax.numel(),
        "measured": msgspec.to_builtins(measured),
        "missing_calibration_refused": True,
        "timing": {"model_calibration_seconds": calibration_seconds,
                   "historical_reference_fit_seconds": historical_fit_seconds,
                   "native_snapshot_fit_pack_write_seconds": conversion_seconds,
                   "checkpoint_adoption_seconds": adoption_seconds},
        "process_peak_rss_bytes": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss * 1024,
        "historical_reference": {
            "producer": "torch-svdq-frozen/1",
            "alpha": old.alpha,
            "calibration_error": old.error,
            "plain_nvfp4_error": old.nvfp4_error,
            "whole_weight_sse": old.sse,
            "whole_weight_energy": old.energy,
        },
    }
    return Converted(True, artifact, json.dumps(detail, sort_keys=True, default=str))


def decoded_weight(parts: Parts) -> torch.Tensor:
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
    from .checks import check_selected

    assert ctx.device.type == "cuda"
    assert payload.conversion, "calibrate's conversion row is required (run.py passes it)"
    conversion = json.loads(payload.conversion)
    row: Row = conversion["row"]
    checkpoint = ctx.tensorfs_source(weights)
    held = checkpoint.components["net"].tensors
    weight = encoded(held.read("proj.weight"))
    assert weight.scheme == "svdq.nvfp4" and weight.shape == (ROWS, COLS) and weight.dtype == "BF16"
    original, original_bias = source_values(payload.seed)
    # The parts the public conversion wrote are exactly the selected fit's: the frozen reference packs its residual.
    assert conversion["new_bytes_written"] == sum(len(part.tobytes()) for part in weight.parts.values())
    facts = check_selected(weight, original.float().numpy(), row)
    reference = decoded_weight(weight.parts)
    bias = tensor(plain(held.read("proj.bias")).tobytes(), torch.bfloat16, (ROWS,))
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
                "stored": facts,
                "cases": rows,
            },
            sort_keys=True,
        ),
    )


class Reuse(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 156


@app.entrypoint(accelerator=False)
async def components(payload: Reuse, ctx: Context) -> Report:
    """Two components converted one call each under one plan identity, then reused and refused from a finished Output:
    the Runtime's built-in `bench.quantize_reuse` (it reopens the Runtime's own Outputs)."""
    found = await call(bench.quantize_reuse, case="components", seed=payload.seed)
    return Report(True, found.detail)


@app.entrypoint(accelerator=False)
async def resume(payload: Reuse, ctx: Context) -> Report:
    """An unfinished Output's close and reopen, finished reuse and identity refusals: `bench.quantize_reuse`."""
    found = await call(bench.quantize_reuse, case="resume", seed=payload.seed)
    return Report(True, found.detail)
