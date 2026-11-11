"""svdq-native, TensorFS's own native SVDQ fitting (public API only), modes basic, cancel and empty-dlpack: `Writer.quantize`
with `svdq-nvfp4-r32-faer/1` on stored checkpoints, every selected residual packed exactly as the independent NVFP4
reference packs it (`nvfp4_encode_oracle.py`), refusals, a real cancel inside a fit and resume, and empty NumPy DLPack
imports. Was the TensorFS half of the gate package `tensorfs/svdq-calibration` (xs-174: a package can't import
TensorFS); run by `release-gate/programs.sh`. The Runtime half (calibration through a model's hooks, the public Output
conversion, a held-out GPU consumer) stays that package."""

from __future__ import annotations

import copy
import gc
import hashlib
import json
import os
import resource
import select
import signal
import subprocess
import sys
import tempfile
import threading
import time
import zlib
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import TYPE_CHECKING, Unpack, cast

import numpy as np
import tensorfs
import torch

from nvfp4_encode_oracle import E2M1_DECODE32, E4M3_MAG32, encode_nvfp4

if TYPE_CHECKING:
    from svdq_native_types import (
        F32,
        ArrayTensor,
        Budget,
        Checkpoint,
        ComponentWriter,
        Control,
        Conversion,
        Converted,
        EncodedTensor,
        Facts,
        Floats,
        MetadataInput,
        Observation,
        Parts,
        Refusing,
        Run,
        Selection,
        Served,
    )

MODES = ("basic", "cancel", "empty-dlpack")
SEED = 256001
STRATEGY = "svdq-nvfp4-r32-faer/1"
WORKSPACE = 64 << 20
# The 1024x1024 fit's grant: since TensorFS's parallel alpha fit (xs-066, 10-28) it needs 69,661,767 bytes.
CANCEL_WORKSPACE = 128 << 20
# What the cancellation cell's conversion reads: its two weights, and each one's 64 calibration rows and maxima.
SOURCE_BYTES = sum(4 * (rows * cols + 64 * cols + cols) for rows, cols in ((64, 256), (1024, 1024)))
PARAMS: "dict[str, MetadataInput]" = {
    "block_size": 16,
    "global_scale": "multiply",
    "nibble_order": "low_first",
    "scale_layout": "row_major",
}


def selection(
    key: str, dtype: str, shape: tuple[int, int], component: str = "weights"
) -> Selection:
    return {
        "component": component,
        "key": key,
        "source_dtype": dtype,
        "shape": list(shape),
        "dtype": dtype,
        "encoding": {"scheme": "svdq.nvfp4", "params": dict(PARAMS)},
        "strategy": STRATEGY,
        "fit": {
            "seed": zlib.crc32(key.encode()),
            "component": component,
            "xmax": key + ".xmax",
            "rows": key + ".rows",
        },
    }


def encoded_bytes(shape: tuple[int, int]) -> int:
    n, k = shape
    return n * k // 2 + n * k // 16 + 8 + k * 4 + (n + k) * 32 * 2


def f32(part: ArrayTensor) -> F32:
    raw = part.tobytes()
    if part.dtype == "BF16":
        return (
            (np.frombuffer(raw, dtype="<u2").astype("<u4") << 16)
            .view("<f4")
            .reshape(part.shape)
        )
    return np.frombuffer(raw, dtype="<f4").reshape(part.shape)


def selected_residual(
    parts: Parts, weight: F32
) -> tuple[F32, F32, F32, F32]:
    """Exact declared new-producer order; never infer the pre-rounding power-law scale.

    The Rust producer first canonicalizes s through stored p: s = F32(1/p). For each
    scalar dot, rank increases 0..31 with a separate F32 multiplication and addition.
    Numpy ufunc boundaries forbid contraction across those operations.
    """
    pre, down, up = (
        f32(parts[role]) for role in ("pre_quant_scale", "lowrank_down", "lowrank_up")
    )
    smooth = np.divide(np.float32(1.0), pre, dtype=np.float32)
    product = np.zeros_like(weight, dtype=np.float32)
    for rank in range(32):
        scaled = np.multiply(down[rank], smooth, dtype=np.float32)
        term = np.multiply(up[:, rank, None], scaled[None, :], dtype=np.float32)
        np.add(product, term, out=product)
    residual = np.subtract(
        np.multiply(weight, smooth, dtype=np.float32), product, dtype=np.float32
    )
    return residual, smooth, up, down


def residual_values(
    parts: Parts, shape: tuple[int, int], dtype: type[np.float32 | np.float64] = np.float64
) -> Floats:
    n, k = shape
    packed = np.frombuffer(parts["weight"].tobytes(), dtype=np.uint8).reshape(n, k // 2)
    codes = np.empty((n, k), dtype=np.uint8)
    codes[:, 0::2], codes[:, 1::2] = packed & 15, packed >> 4
    scales = np.frombuffer(parts["scale"].tobytes(), dtype=np.uint8).reshape(n, k // 16)
    second: float = f32(parts["scale_2"]).item()
    # Decoder evaluation is (code*scale)*global, rather than code*(scale*global).
    return cast("Floats", (  # cast: astype of a dtype union
        E2M1_DECODE32[codes].astype(dtype)
        * np.repeat(E4M3_MAG32[scales].astype(dtype), 16, axis=1)  # type: ignore[operator]  # numpy's stubs: a product of a dtype union's astype
    ) * second)


def stored_values(parts: Parts, shape: tuple[int, int]) -> F32:
    """Existing decoder order: F32 code*scale*global*pre, then rank-ordered F32 adds."""
    pre, down, up = (
        f32(parts[role]) for role in ("pre_quant_scale", "lowrank_down", "lowrank_up")
    )
    values: F32 = np.multiply(
        residual_values(parts, shape, np.float32), pre, dtype=np.float32
    )
    for rank in range(32):
        term = np.multiply(up[:, rank, None], down[rank, None, :], dtype=np.float32)
        np.add(values, term, out=values)
    return values


def _fake(xs: torch.Tensor, g: torch.Tensor) -> torch.Tensor:
    """The frozen serving law's activation fake-quantization (`svdq-calibration`'s `historical_svdq._fake`)."""
    blocks = xs.view(xs.shape[0], -1, 16)
    amax = blocks.abs().amax(-1)
    scale = (amax / (6.0 * g)).clamp(2.0**-9, 448.0).to(torch.float8_e4m3fn)
    a = (blocks / (scale.float() * g).unsqueeze(-1)).abs()
    code = cast("torch.Tensor", sum(  # cast: seven Tensor terms, never the empty sum's 0
        ((a >= t) if odd else (a > t)).to(torch.uint8)
        for t, odd in ((0.25, 0), (0.75, 1), (1.25, 0), (1.75, 1), (2.5, 0), (3.5, 1), (5.0, 0))
    ))
    code = code + (blocks < 0).to(torch.uint8) * 8
    table = torch.tensor(E2M1_DECODE32, dtype=torch.float32, device=xs.device)
    return (table[code.int()] * scale.float().unsqueeze(-1) * g.float().reshape(())).view(xs.shape)


def served_score(
    parts: Parts, weight: F32, calibration: F32
) -> Served:
    """Independent frozen serving law uses rows*stored_pre, never rows/reconstructed_s."""
    rows = torch.from_numpy(calibration.copy()).float()
    pre = torch.from_numpy(f32(parts["pre_quant_scale"]).copy())
    down = torch.from_numpy(f32(parts["lowrank_down"]).copy())
    up = torch.from_numpy(f32(parts["lowrank_up"]).copy())
    gx = torch.from_numpy(f32(parts["input_scale"]).copy())
    decoded = torch.from_numpy(residual_values(parts, weight.shape, np.float32))
    scaled = (rows * pre).bfloat16().float()
    alternate = (rows / (1.0 / pre)).bfloat16().float()
    prediction = _fake(scaled, gx) @ decoded.t()
    prediction += (rows @ down.t()).bfloat16().float() @ up.t()
    reference = rows @ torch.from_numpy(weight).t()
    difference = (prediction - reference).double()
    denominator = torch.linalg.vector_norm(reference.double(), dim=1).clamp(min=1e-30)
    error = torch.linalg.vector_norm(difference, dim=1) / denominator
    return {
        "stored_pre_calibration_error": float(error.mean()),
        "multiply_vs_divide_bf16_differences": int((scaled != alternate).sum()),
    }


def check_selected(
    item: EncodedTensor,
    weight: F32,
    measured: Converted,
    *,
    exact_svd: bool = True,
    calibration: F32 | None = None,
) -> Facts:
    n, k = weight.shape
    expected_geometry = {
        "weight": ("F4", (n, k)),
        "scale": ("F8_E4M3", (n, k // 16)),
        "scale_2": ("F32", ()),
        "pre_quant_scale": ("F32", (k,)),
        "lowrank_down": ("BF16", (32, k)),
        "lowrank_up": ("BF16", (n, 32)),
        "input_scale": ("F32", ()),
    }
    assert set(item.parts) == set(expected_geometry)
    for role, geometry in expected_geometry.items():
        assert (item.parts[role].dtype, tuple(item.parts[role].shape)) == geometry, role
    residual, smooth, up, down = selected_residual(item.parts, weight)
    reference = encode_nvfp4("selected-native-residual", residual)
    assert item.parts["weight"].tobytes() == reference.payload
    assert item.parts["scale"].tobytes() == reference.companions[0].raw
    assert item.parts["scale_2"].tobytes() == reference.companions[1].raw
    fit = measured["fit"]
    assert fit["strategy"] == STRATEGY
    assert (
        min(
            abs(fit["alpha"] - alpha)
            for alpha in (0.0, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8)
        )
        <= 1e-7
    )
    assert fit["residual"]["saturated"] == reference.saturated
    assert np.isclose(
        fit["residual"]["energy"], reference.energy, rtol=1e-12, atol=1e-18
    )
    assert fit["residual"]["amax"] == float(np.abs(residual).max())
    decoded = stored_values(item.parts, weight.shape)
    error = decoded.astype(np.float64) - weight.astype(np.float64)
    energy = float(np.square(weight.astype(np.float64)).sum())
    sse = float(np.square(error).sum())
    assert np.isfinite(decoded).all()
    assert np.isclose(measured["measured"]["energy"], energy, rtol=1e-12, atol=1e-18)
    assert np.isclose(measured["measured"]["sse"], sse, rtol=1e-10, atol=1e-16)
    relative = np.sqrt(sse / energy) if energy else 0.0
    assert np.isclose(measured["relative_frobenius"], relative, rtol=1e-10, atol=1e-14)
    assert np.isfinite(fit["calibration_error"]) and np.isfinite(
        fit["plain_nvfp4_error"]
    )
    assert np.all(np.isfinite(smooth)) and np.all(smooth > 0)
    facts: Facts = {
        "fit": fit,
        "relative_frobenius": relative,
        "logical_energy": energy,
        "residual_energy": reference.energy,
        "residual_packing_sha256": hashlib.sha256(reference.payload).hexdigest(),
        "stored_parts": {
            role: hashlib.sha256(part.tobytes()).hexdigest()
            for role, part in item.parts.items()
        },
    }
    if calibration is not None:
        served = served_score(item.parts, weight, calibration)
        assert np.isclose(
            fit["calibration_error"],
            served["stored_pre_calibration_error"],
            rtol=2e-5,
            atol=2e-7,
        ), (fit["calibration_error"], served)
        facts.update(served)  # type: ignore[typeddict-item]  # Served is a partial Facts
    if exact_svd:
        target = weight.astype(np.float64) * smooth
        approximation = up.astype(np.float64) @ (down.astype(np.float64) * smooth)
        actual = np.linalg.norm(target - approximation)
        singular = np.linalg.svd(target, compute_uv=False)
        optimal = np.linalg.norm(singular[32:])
        # Quality comparison is about reconstructed subspaces, never singular-vector signs.
        assert actual <= optimal * 1.15 + np.linalg.norm(target) * 0.015, (
            actual,
            optimal,
        )
        facts["lowrank_error"] = float(actual)
        facts["exact_rank32_error"] = float(optimal)
    return facts


def put_source(component: ComponentWriter, key: str, weight: F32, dtype: str) -> F32:
    if dtype == "BF16":
        t = torch.from_numpy(weight).bfloat16()
        raw, held = t.view(torch.uint8).numpy().tobytes(), t.float().numpy()
    elif dtype == "F16":
        held = weight.astype("<f2")
        raw, held = held.tobytes(), held.astype("<f4")
    else:
        held, raw = weight.astype("<f4"), weight.astype("<f4").tobytes()
    component.put(key, raw, dtype=dtype, shape=list(weight.shape))
    return held


def calibration_writer(
    path: Path,
    cases: dict[str, F32],
    rows: dict[str, F32],
    *,
    change: bool = False,
) -> Checkpoint:
    writer = tensorfs.Writer(path)
    component = writer.component("weights")
    for key in cases:
        samples = rows[key]
        maxima = np.abs(samples).max(0).astype("<f4")
        if change:
            maxima[0] = np.nextafter(maxima[0], np.float32(np.inf))
        component.put(key + ".xmax", maxima)
        component.put(key + ".rows", samples)
    return writer.commit()


def refused(
    directory: Path,
    label: str,
    source: Checkpoint,
    specs: list[Selection],
    calibration: Checkpoint | None,
    **options: Unpack[Refusing],
) -> str:
    output = tensorfs.Writer(directory / (label + ".cozytensors"), base=source)
    control = options.pop("control", tensorfs.ConversionControl())
    try:
        output.quantize(
            source,
            specs,
            calibration=calibration,
            control=control,
            **cast("Budget", {"max_buffer_bytes": WORKSPACE, "max_new_bytes": 16 << 20, **options}),  # cast: control popped
        )
    except tensorfs.Error as error:
        assert not any(output.has(spec["component"], spec["key"]) for spec in specs)
        return str(error)
    finally:
        output.abandon()
    raise AssertionError(label + " unexpectedly published")


def basic(seed: int) -> dict[str, object]:
    rng = np.random.default_rng(seed)
    with tempfile.TemporaryDirectory(prefix="cozy-native-svdq-") as temporary:
        directory = Path(temporary)
        writer = tensorfs.Writer(directory / "source.cozytensors")
        component = writer.component("weights", config={"inherited": True})
        component.put("bias", np.arange(128, dtype="<f4"))
        raw = rng.normal(0, 0.0625, (128, 256)).astype("<f4")
        outlier = raw.copy()
        outlier[:, 0] *= 64
        outlier[-1, -1] = 16
        deficient = (rng.normal(size=(128, 8)) @ rng.normal(size=(8, 256))).astype(
            "<f4"
        ) * 0.05
        inputs = [
            ("f32", raw, "F32"),
            ("f16", raw, "F16"),
            ("bf16", raw, "BF16"),
            ("zero", np.zeros_like(raw), "F32"),
            ("rankdef", deficient, "F32"),
            ("outlier", outlier, "F32"),
        ]
        cases: dict[str, F32] = {}
        specs: list[Selection] = []
        samples: dict[str, F32] = {}
        for key, weight, dtype in inputs:
            cases[key] = put_source(component, key, weight, dtype)
            specs.append(selection(key, dtype, weight.shape))
            samples[key] = (
                rng.normal(0, 0.125, (96, 256)) * np.linspace(0.25, 2, 256)
            ).astype("<f4")
        source = writer.commit()
        calibration = calibration_writer(
            directory / "calibration.cozytensors", cases, samples
        )
        allowance = sum(encoded_bytes(w.shape) for w in cases.values())
        controls: list[Control] = []
        reports: list[Run] = []
        baseline: dict[str, dict[str, bytes]] | None = None
        for index, workspace in enumerate((WORKSPACE, WORKSPACE * 2)):
            output = tensorfs.Writer(
                directory / f"fitted-{index}.cozytensors", base=source
            )
            control = tensorfs.ConversionControl()
            controls.append(control)
            started = time.perf_counter()
            result: "Conversion" = output.quantize(
                source,
                specs,
                calibration=calibration,
                max_buffer_bytes=workspace,
                max_new_bytes=allowance,
                control=control,
            )
            fit_seconds = time.perf_counter() - started
            checkpoint = output.commit()
            assert checkpoint.verify().ok
            assert result["encoded_keys"] == len(cases) and result["reused_keys"] == 0
            assert result["new_bytes_written"] == allowance
            expected_read = sum(
                w.size * (4 if dtype == "F32" else 2) for _, w, dtype in inputs
            )
            assert result["source_bytes_read"] == expected_read
            by_key = {entry["key"]: entry for entry in result["tensors"]}
            facts: dict[str, Facts] = {}
            encoded: dict[str, dict[str, bytes]] = {}
            for key, weight in cases.items():
                item = checkpoint.components["weights"].tensors.read(key)
                assert isinstance(item, tensorfs.Encoded)
                facts[key] = check_selected(
                    item,
                    weight,
                    by_key[key],
                    exact_svd=index == 0,
                    calibration=samples[key],
                )
                assert (
                    by_key[key]["fit"]["calibration_bytes_read"]
                    == (samples[key].size + weight.shape[1]) * 4
                )
                encoded[key] = {
                    role: part.tobytes() for role, part in item.parts.items()
                }
                if key == "zero":
                    assert by_key[key]["measured"]["sse"] == 0
                    assert by_key[key]["fit"]["calibration_error"] == 0
                    assert (
                        np.count_nonzero(
                            np.frombuffer(encoded[key]["weight"], dtype=np.uint8)
                        )
                        == 0
                    )
            if baseline is None:
                baseline = encoded
            else:
                assert encoded == baseline, (
                    "workspace allowance changed deterministic bytes"
                )
                assert result["identity"] == reports[0]["result"]["identity"]
            assert (
                cast("ArrayTensor", checkpoint.components["weights"].tensors.read("bias")).tobytes()  # cast: plain
                == np.arange(128, dtype="<f4").tobytes()
            )
            reports.append(
                {
                    "workspace": workspace,
                    "fit_seconds": fit_seconds,
                    "result": result,
                    "facts": facts,
                    "checkpoint": checkpoint.id,
                }
            )
            checkpoint.close()

        one = specs[:1]
        refusals = {
            "unknown_component": refused(
                directory,
                "unknown-component",
                source,
                one,
                calibration,
                component="missing",
            ),
            "workspace": refused(
                directory,
                "workspace",
                source,
                one,
                calibration,
                max_buffer_bytes=1,
            ),
            "output": refused(
                directory, "output", source, one, calibration, max_new_bytes=1
            ),
            "quality": refused(
                directory,
                "quality",
                source,
                one,
                calibration,
                max_relative_frobenius=0,
            ),
            "missing_calibration": refused(
                directory, "missing-calibration", source, one, None
            ),
        }
        before = tensorfs.ConversionControl()
        before.cancel()
        refusals["pre_cancel"] = refused(
            directory, "pre-cancel", source, one, calibration, control=before
        )
        assert before.progress() == (0, 0, 0)
        changed = copy.deepcopy(one)
        cast("list[int]", changed[0]["shape"])[1] -= 1  # cast: selection() builds it a list
        refusals["shape"] = refused(
            directory, "shape", source, changed, calibration
        )
        changed = copy.deepcopy(one)
        changed[0]["strategy"] = "svdq-nvfp4-r32-faer/unknown"
        refusals["strategy"] = refused(
            directory, "strategy", source, changed, calibration
        )
        changed = copy.deepcopy(one)
        changed[0]["fit"]["rows"] = "f32.xmax"
        refusals["role_shape"] = refused(
            directory, "role-shape", source, changed, calibration
        )
        for label, which, value in (
            ("nan_weight", "weight", np.nan),
            ("inf_rows", "rows", np.inf),
            ("nan_xmax", "xmax", np.nan),
            ("negative_xmax", "xmax", -1.0),
        ):
            w = tensorfs.Writer(directory / (label + "-source.cozytensors"))
            c = w.component("weights")
            bad_weight = cases["f32"].copy()
            if which == "weight":
                bad_weight[0, 0] = value
            c.put("f32", bad_weight)
            bad_source = w.commit()
            w = tensorfs.Writer(directory / (label + "-calibration.cozytensors"))
            c = w.component("weights")
            x = np.abs(samples["f32"]).max(0).astype("<f4")
            r = samples["f32"].copy()
            if which == "rows":
                r[0, 0] = value
            if which == "xmax":
                x[0] = value
            c.put("f32.xmax", x)
            c.put("f32.rows", r)
            bad_calibration = w.commit()
            refusals[label] = refused(
                directory, label, bad_source, one, bad_calibration
            )
            bad_source.close()
            bad_calibration.close()
        # Empty calibration and unsupported geometry are represented by real checkpoints.
        for label, shape in (
            ("empty_calibration", (0, 256)),
            ("wrong_calibration_width", (96, 128)),
        ):
            w = tensorfs.Writer(directory / (label + "-input.cozytensors"))
            c = w.component("weights")
            c.put("f32.xmax", np.ones(256, dtype="<f4"))
            c.put("f32.rows", np.ones(shape, dtype="<f4"))
            bad = w.commit()
            assert tuple(bad.components["weights"].tensors["f32.rows"].shape) == shape
            refusals[label] = refused(directory, label, source, one, bad)
            assert "SVDQ calibration rows must be nonempty and match the weight columns" in refusals[label]
            bad.close()
        source.close()
        calibration.close()
        return {
            "reports": reports,
            "refusals": refusals,
            "implementation": tensorfs.raw.quantization_implementation(),
            "tensorfs": tensorfs.__version__,
            "scope": "stored-checkpoint CPU numerical proof; no GPU consumer or speed claim",
        }


def cancel(seed: int) -> dict[str, object]:
    """Cancels a real conversion inside its native fit, every time: the fit can end before a poll here
    sees its window (gates 41911483592, 41939634423), so the conversion runs in a worker that this
    process holds with SIGSTOP once the worker's progress shows the first tensor written and every
    read of the second done, and only then tells it to cancel. No timing is tuned: the worker is
    stopped where the fit is, and its conversion threads run at nice 19 under its monitor."""
    with tempfile.TemporaryFile("w+") as errors:
        worker = subprocess.Popen(
            [sys.executable, "-s", "-E", str(Path(__file__).resolve()), "cancel-worker", str(seed)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors, text=True
        )
        watchdog = threading.Timer(300, worker.kill)
        watchdog.start()
        held: list[int] | None = None
        assert worker.stdout is not None and worker.stdin is not None
        try:
            for line in worker.stdout:
                word, _, rest = line.partition(" ")
                if word == "early":
                    raise AssertionError(f"the conversion ended before its cancellation window: {rest.strip()[:400]}")
                if word == "result":
                    result: dict[str, object] = json.loads(rest)
                    assert worker.wait(timeout=60) == 0
                    result["held_at_progress"] = held
                    result["worker_peak_rss_bytes"] = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss * 1024
                    return result
                if word == "progress" and held is None:
                    read, written, _ = map(int, rest.split())
                    if written >= 1 and read >= SOURCE_BYTES:
                        os.kill(worker.pid, signal.SIGSTOP)
                        _, status = os.waitpid(worker.pid, os.WUNTRACED)
                        assert os.WIFSTOPPED(status), status
                        held = [read, written]
                        worker.stdin.write("cancel\n")
                        worker.stdin.flush()
                        os.kill(worker.pid, signal.SIGCONT)
            errors.seek(0)
            raise AssertionError(f"the worker ended with {worker.wait()} and no result: {errors.read()[-1500:]}")
        finally:
            watchdog.cancel()
            if worker.poll() is None:
                worker.kill()
                worker.wait()


def cancel_worker(seed: int) -> None:
    """The cancellation cell's conversion, in a process `cancel` holds from outside: see there."""
    rng = np.random.default_rng(seed)
    with tempfile.TemporaryDirectory(prefix="cozy-native-svdq-cancel-") as temporary:
        directory = Path(temporary)
        w = tensorfs.Writer(directory / "source.cozytensors")
        c = w.component("weights")
        cases = {
            "first": np.zeros((64, 256), dtype="<f4"),
            "large": rng.normal(0, 0.1, (1024, 1024)).astype("<f4"),
        }
        samples = {
            key: rng.normal(0, 0.1, (64, values.shape[1])).astype("<f4")
            for key, values in cases.items()
        }
        for key, values in cases.items():
            c.put(key, values)
        source = w.commit()
        calibration = calibration_writer(
            directory / "calibration.cozytensors", cases, samples
        )
        changed_calibration = calibration_writer(
            directory / "changed.cozytensors", cases, samples, change=True
        )
        specs = [selection(key, "F32", values.shape) for key, values in cases.items()]
        allowance = sum(encoded_bytes(value.shape) for value in cases.values())
        target = directory / "output.cozytensors"
        resume = f"native-svdq-{seed}"
        output = tensorfs.Writer(target, base=source, resume=resume)
        control = tensorfs.ConversionControl()
        monitor = threading.get_native_id()

        def convert() -> "Conversion":
            # Every thread but the monitor's runs at nice 19, and those the conversion starts inherit it.
            for task in os.listdir("/proc/self/task"):
                if int(task) != monitor:
                    os.setpriority(os.PRIO_PROCESS, int(task), 19)
            return output.quantize(
                source,
                specs,
                calibration=calibration,
                max_buffer_bytes=CANCEL_WORKSPACE,
                max_new_bytes=allowance,
                control=control,
            )

        with ThreadPoolExecutor(max_workers=1) as pool:
            future = pool.submit(convert)
            # Progress out, until `cancel` has held this process and says to cancel. No hook or mock advances fitting.
            seen: tuple[int, int, int] | None = None
            while not select.select([sys.stdin], [], [], 0.0002)[0]:
                if future.done():
                    print("early", repr(future.exception() or future.result()), flush=True)
                    return
                if control.progress() != seen:
                    seen = control.progress()
                    print("progress", *seen, flush=True)
            assert sys.stdin.readline() == "cancel\n"
            for operation in (output.commit, output.abandon):
                try:
                    operation()
                except tensorfs.ConflictError:
                    pass
                else:
                    raise AssertionError("busy Writer did not refuse")
            started = time.perf_counter()
            control.cancel()
            try:
                future.result(timeout=120)
            except tensorfs.NetworkError as error:
                assert "stopped" in str(error)
            else:
                raise AssertionError("native fitting completed before cancellation")
            cancel_seconds = time.perf_counter() - started
        assert output.has("weights", "first") and not output.has("weights", "large")
        del operation, future, output
        gc.collect()
        reopened = tensorfs.Writer(target, base=source, resume=resume)
        changed = copy.deepcopy(specs)
        changed[0]["fit"]["seed"] += 1
        refusals: list[str] = []
        for rows, calibration_arg in (
            (changed, calibration),
            (specs, changed_calibration),
        ):
            try:
                reopened.quantize(
                    source,
                    rows,
                    calibration=calibration_arg,
                    max_buffer_bytes=CANCEL_WORKSPACE,
                    max_new_bytes=allowance,
                    control=tensorfs.ConversionControl(),
                )
            except tensorfs.InvalidError as error:
                assert "selected by another conversion" in str(error)
                refusals.append(str(error))
            else:
                raise AssertionError("resume accepted a changed calibration/seed")
            assert reopened.has("weights", "first") and not reopened.has(
                "weights", "large"
            )
        resume_began = time.perf_counter()
        result: "Conversion" = reopened.quantize(
            source,
            specs,
            calibration=calibration,
            max_buffer_bytes=CANCEL_WORKSPACE,
            max_new_bytes=allowance,
            control=tensorfs.ConversionControl(),
        )
        resume_fit_seconds = time.perf_counter() - resume_began
        assert result["reused_keys"] == 1 and result["encoded_keys"] == 1
        assert result["worst_relative_frobenius"] is None
        checkpoint = reopened.commit()
        assert checkpoint.verify().ok
        item = cast("EncodedTensor", checkpoint.components["weights"].tensors.read("large"))  # cast: quantized
        facts = check_selected(
            item,
            cases["large"],
            result["tensors"][0],
            exact_svd=False,
            calibration=samples["large"],
        )
        checkpoint.close()
        source.close()
        calibration.close()
        changed_calibration.close()
        print("result", json.dumps({
            "result": result,
            "facts": facts,
            "cancel_seconds": cancel_seconds,
            "resume_fit_seconds": resume_fit_seconds,
            "cancel_progress": control.progress(),
            "identity_refusals": refusals,
            "scope": "real interrupted/resumed Writer; cancellation waits for any current faer call",
        }), flush=True)


def empty_dlpack() -> dict[str, object]:
    """Real Writer imports of empty NumPy DLPack arrays, plus nonempty stride refusals."""
    observations: list[Observation] = []
    with tempfile.TemporaryDirectory(prefix="cozy-empty-dlpack-") as temporary:
        writer = tensorfs.Writer(Path(temporary) / "empty.cozytensors")
        component = writer.component("weights")
        for index, shape in enumerate(((0, 256), (3, 0), (1, 0, 256), (0,), (1, 256))):
            array = np.ones(shape, dtype="<f4")
            component.put(str(index), array)
            observations.append({"key": str(index), "shape": shape, "strides": array.strides,
                                 "c_contiguous": bool(array.flags.c_contiguous), "bytes": array.nbytes})
        strided = np.ones((3, 512), dtype="<f4")[:, ::2]
        try:
            component.put("strided", strided)
        except tensorfs.InvalidError as error:
            assert "contiguous DLPack" in str(error)
        else:
            raise AssertionError("nonempty noncontiguous source was accepted")
        assert not component.has("strided")
        checkpoint = writer.commit()
        assert checkpoint.verify().ok
        for row in observations:
            value = cast("ArrayTensor", checkpoint.components["weights"].tensors.read(row["key"]))  # cast: plain
            assert tuple(value.shape) == tuple(row["shape"])
            assert value.tobytes() == np.ones(row["shape"], dtype="<f4").tobytes()
        checkpoint.close()
    return {"arrays": observations, "nonempty_strided_refused": True, "tensorfs": tensorfs.__version__}


def run(mode: str, seed: int) -> dict[str, object]:
    began = time.perf_counter()
    rss_before = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss * 1024
    result = basic(seed) if mode == "basic" else cancel(seed) if mode == "cancel" else empty_dlpack()
    result["gate_wall_seconds"] = time.perf_counter() - began
    result["process_peak_rss_bytes"] = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss * 1024
    result["process_peak_rss_before_bytes"] = rss_before
    result["memory_scope"] = "Linux process high-water RSS including imports and independent references; not fitter-only peak"
    return result


if __name__ == "__main__":
    import traceback

    if sys.argv[1:2] == ["cancel-worker"]:
        cancel_worker(int(sys.argv[2]))
        sys.exit(0)
    seed = SEED
    for mode in MODES:
        cell = "svdq-native" if mode == MODES[0] else f"svdq-native-{mode}"
        try:
            row = {"cell": cell, "result": "PASS", "detail": json.dumps({"seed": seed, **run(mode, seed)}, sort_keys=True, default=str)}
        except Exception as error:  # noqa: BLE001 - any failure is this mode's verdict
            row = {"cell": cell, "result": "FAIL", "detail": f"seed {seed}: {type(error).__name__}: {error}"[:600]}
            traceback.print_exc()
        print("ROW " + json.dumps(row, sort_keys=True, default=str), flush=True)
