"""Actual stored-checkpoint native fitting, independent arithmetic and lifetime gates."""

from __future__ import annotations

import copy
import gc
import hashlib
import json
import resource
import tempfile
import time
import zlib
from collections import OrderedDict
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

import msgspec
import numpy as np
import torch
from cozy_runtime.author import Context, ModelArtifact, Telemetry, WeightsOutput

from . import Report, Request, app
from .oracle import E2M1_DECODE32, E4M3_MAG32, encode_nvfp4

STRATEGY = "svdq-nvfp4-r32-faer/1"
WORKSPACE = 64 << 20
PARAMS = {
    "block_size": 16,
    "global_scale": "multiply",
    "nibble_order": "low_first",
    "scale_layout": "row_major",
}


def selection(
    key: str, dtype: str, shape: tuple[int, int], component: str = "weights"
) -> dict[str, Any]:
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


def f32(part: Any) -> np.ndarray:
    raw = part.tobytes()
    if part.dtype == "BF16":
        return (
            (np.frombuffer(raw, dtype="<u2").astype("<u4") << 16)
            .view("<f4")
            .reshape(part.shape)
        )
    return np.frombuffer(raw, dtype="<f4").reshape(part.shape)


def selected_residual(
    parts: Any, weight: np.ndarray
) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
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
    parts: Any, shape: tuple[int, int], dtype: Any = np.float64
) -> np.ndarray:
    n, k = shape
    packed = np.frombuffer(parts["weight"].tobytes(), dtype=np.uint8).reshape(n, k // 2)
    codes = np.empty((n, k), dtype=np.uint8)
    codes[:, 0::2], codes[:, 1::2] = packed & 15, packed >> 4
    scales = np.frombuffer(parts["scale"].tobytes(), dtype=np.uint8).reshape(n, k // 16)
    second = f32(parts["scale_2"]).item()
    # Decoder evaluation is (code*scale)*global, rather than code*(scale*global).
    return (
        E2M1_DECODE32[codes].astype(dtype)
        * np.repeat(E4M3_MAG32[scales].astype(dtype), 16, axis=1)
    ) * second


def stored_values(parts: Any, shape: tuple[int, int]) -> np.ndarray:
    """Existing decoder order: F32 code*scale*global*pre, then rank-ordered F32 adds."""
    pre, down, up = (
        f32(parts[role]) for role in ("pre_quant_scale", "lowrank_down", "lowrank_up")
    )
    values = np.multiply(
        residual_values(parts, shape, np.float32), pre, dtype=np.float32
    )
    for rank in range(32):
        term = np.multiply(up[:, rank, None], down[rank, None, :], dtype=np.float32)
        np.add(values, term, out=values)
    return values


def served_score(
    parts: Any, weight: np.ndarray, calibration: np.ndarray
) -> dict[str, Any]:
    """Independent frozen serving law uses rows*stored_pre, never rows/reconstructed_s."""
    from .historical_svdq import _fake

    rows = torch.from_numpy(calibration.copy()).float()
    pre = torch.from_numpy(f32(parts["pre_quant_scale"]).copy())
    down = torch.from_numpy(f32(parts["lowrank_down"]).copy())
    up = torch.from_numpy(f32(parts["lowrank_up"]).copy())
    gx = torch.from_numpy(f32(parts["input_scale"]).copy())
    decoded = torch.from_numpy(residual_values(parts, weight.shape, np.float32))
    scaled = (rows * pre).bfloat16().float()
    alternate = (rows / (1.0 / pre)).bfloat16().float()
    prediction = _fake(torch, scaled, gx) @ decoded.t()
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
    item: Any,
    weight: np.ndarray,
    measured: dict[str, Any],
    *,
    exact_svd: bool = True,
    calibration: np.ndarray | None = None,
) -> dict[str, Any]:
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
    facts = {
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
        facts.update(served)
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


def put_source(component: Any, key: str, weight: np.ndarray, dtype: str) -> np.ndarray:
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
    tensorfs: Any,
    path: Path,
    cases: dict[str, np.ndarray],
    rows: dict[str, np.ndarray],
    *,
    change: bool = False,
) -> Any:
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
    tensorfs: Any,
    directory: Path,
    label: str,
    source: Any,
    specs: list[dict[str, Any]],
    calibration: Any,
    **options: Any,
) -> str:
    output = tensorfs.Writer(directory / (label + ".cozytensors"), base=source)
    control = options.pop("control", tensorfs.ConversionControl())
    try:
        output.quantize(
            source,
            specs,
            calibration=calibration,
            control=control,
            **{"max_buffer_bytes": WORKSPACE, "max_new_bytes": 16 << 20, **options},
        )
    except tensorfs.Error as error:
        assert not any(output.has(spec["component"], spec["key"]) for spec in specs)
        return str(error)
    finally:
        output.abandon()
    raise AssertionError(label + " unexpectedly published")


def basic(payload: Request) -> dict[str, Any]:
    import tensorfs

    rng = np.random.default_rng(payload.seed)
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
        cases, specs, samples = {}, [], {}
        for key, weight, dtype in inputs:
            cases[key] = put_source(component, key, weight, dtype)
            specs.append(selection(key, dtype, weight.shape))
            samples[key] = (
                rng.normal(0, 0.125, (96, 256)) * np.linspace(0.25, 2, 256)
            ).astype("<f4")
        source = writer.commit()
        calibration = calibration_writer(
            tensorfs, directory / "calibration.cozytensors", cases, samples
        )
        allowance = sum(encoded_bytes(w.shape) for w in cases.values())
        controls = []
        reports, baseline = [], None
        for index, workspace in enumerate((WORKSPACE, WORKSPACE * 2)):
            output = tensorfs.Writer(
                directory / f"fitted-{index}.cozytensors", base=source
            )
            control = tensorfs.ConversionControl()
            controls.append(control)
            started = time.perf_counter()
            result = output.quantize(
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
            facts, encoded = {}, {}
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
                checkpoint.components["weights"].tensors.read("bias").tobytes()
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

        # Independent frozen Torch fitter is a quality baseline, never a fallback or byte oracle.
        from .historical_svdq import Stats, fit

        old = fit(
            torch,
            "f32",
            cases["f32"],
            Stats(
                torch.from_numpy(np.abs(samples["f32"]).max(0)),
                torch.from_numpy(samples["f32"]),
            ),
            "cpu",
        )
        new = reports[0]["facts"]["f32"]["fit"]
        assert new["calibration_error"] <= old.error * 1.25 + 0.02, (new, old.error)
        historical = {
            "calibration_error": old.error,
            "plain_nvfp4_error": old.nvfp4_error,
            "alpha": old.alpha,
            "whole_weight_relative": (old.sse / old.energy) ** 0.5,
        }
        one = specs[:1]
        refusals = {
            "unknown_component": refused(
                tensorfs,
                directory,
                "unknown-component",
                source,
                one,
                calibration,
                component="missing",
            ),
            "workspace": refused(
                tensorfs,
                directory,
                "workspace",
                source,
                one,
                calibration,
                max_buffer_bytes=1,
            ),
            "output": refused(
                tensorfs, directory, "output", source, one, calibration, max_new_bytes=1
            ),
            "quality": refused(
                tensorfs,
                directory,
                "quality",
                source,
                one,
                calibration,
                max_relative_frobenius=0,
            ),
            "missing_calibration": refused(
                tensorfs, directory, "missing-calibration", source, one, None
            ),
        }
        before = tensorfs.ConversionControl()
        before.cancel()
        refusals["pre_cancel"] = refused(
            tensorfs, directory, "pre-cancel", source, one, calibration, control=before
        )
        assert before.progress() == (0, 0, 0)
        changed = copy.deepcopy(one)
        changed[0]["shape"][1] -= 1
        refusals["shape"] = refused(
            tensorfs, directory, "shape", source, changed, calibration
        )
        changed = copy.deepcopy(one)
        changed[0]["strategy"] = "svdq-nvfp4-r32-faer/unknown"
        refusals["strategy"] = refused(
            tensorfs, directory, "strategy", source, changed, calibration
        )
        changed = copy.deepcopy(one)
        changed[0]["fit"]["rows"] = "f32.xmax"
        refusals["role_shape"] = refused(
            tensorfs, directory, "role-shape", source, changed, calibration
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
                tensorfs, directory, label, bad_source, one, bad_calibration
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
            refusals[label] = refused(tensorfs, directory, label, source, one, bad)
            assert "SVDQ calibration rows must be nonempty and match the weight columns" in refusals[label]
            bad.close()
        source.close()
        calibration.close()
        return {
            "reports": reports,
            "historical_reference": historical,
            "refusals": refusals,
            "implementation": tensorfs.raw.quantization_implementation(),
            "tensorfs": tensorfs.__version__,
            "scope": "stored-checkpoint CPU numerical proof; no GPU consumer or speed claim",
        }


def cancel(payload: Request) -> dict[str, Any]:
    import tensorfs

    rng = np.random.default_rng(payload.seed)
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
            tensorfs, directory / "calibration.cozytensors", cases, samples
        )
        changed_calibration = calibration_writer(
            tensorfs, directory / "changed.cozytensors", cases, samples, change=True
        )
        specs = [selection(key, "F32", values.shape) for key, values in cases.items()]
        allowance = sum(encoded_bytes(value.shape) for value in cases.values())
        target = directory / "output.cozytensors"
        resume = f"native-svdq-{payload.seed}"
        output = tensorfs.Writer(target, base=source, resume=resume)
        control = tensorfs.ConversionControl()
        with ThreadPoolExecutor(max_workers=1) as pool:
            future = pool.submit(
                output.quantize,
                source,
                specs,
                calibration=calibration,
                max_buffer_bytes=WORKSPACE,
                max_new_bytes=allowance,
                control=control,
            )
            until = time.monotonic() + 120
            # Observe a real first tensor and all second weight reads. No hook or mock advances fitting.
            source_bytes = sum(v.nbytes for v in cases.values()) + sum(
                r.nbytes + r.shape[1] * 4 for r in samples.values()
            )
            while control.progress()[1] < 1 or control.progress()[0] < source_bytes:
                assert not future.done(), "the actual cancellation window was missed"
                assert time.monotonic() < until, control.progress()
                time.sleep(0.001)
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
        refusals = []
        for rows, calibration_arg in (
            (changed, calibration),
            (specs, changed_calibration),
        ):
            try:
                reopened.quantize(
                    source,
                    rows,
                    calibration=calibration_arg,
                    max_buffer_bytes=WORKSPACE,
                    max_new_bytes=allowance,
                    control=tensorfs.ConversionControl(),
                )
            except tensorfs.InvalidError as error:
                assert "recipe changed" in str(error)
                refusals.append(str(error))
            else:
                raise AssertionError("resume accepted a changed calibration/seed")
            assert reopened.has("weights", "first") and not reopened.has(
                "weights", "large"
            )
        result = reopened.quantize(
            source,
            specs,
            calibration=calibration,
            max_buffer_bytes=WORKSPACE,
            max_new_bytes=allowance,
            control=tensorfs.ConversionControl(),
        )
        assert result["reused_keys"] == 1 and result["encoded_keys"] == 1
        assert result["worst_relative_frobenius"] is None
        checkpoint = reopened.commit()
        assert checkpoint.verify().ok
        item = checkpoint.components["weights"].tensors.read("large")
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
        return {
            "result": result,
            "facts": facts,
            "cancel_seconds": cancel_seconds,
            "cancel_progress": control.progress(),
            "identity_refusals": refusals,
            "scope": "real interrupted/resumed Writer; cancellation waits for any current faer call",
        }


@app.entrypoint(accelerator=False)
def native(payload: Request) -> Report:
    assert payload.mode in ("basic", "cancel")
    began = time.perf_counter()
    rss_before = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss * 1024
    result = basic(payload) if payload.mode == "basic" else cancel(payload)
    result["gate_wall_seconds"] = time.perf_counter() - began
    result["process_peak_rss_bytes"] = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss * 1024
    result["process_peak_rss_before_bytes"] = rss_before
    result["memory_scope"] = "Linux process high-water RSS including imports and independent references; not fitter-only peak"
    return Report(True, json.dumps(result, sort_keys=True))


@app.entrypoint(accelerator=False)
def empty_dlpack(payload: Request) -> Report:
    """Real Writer imports of empty NumPy DLPack arrays, plus nonempty stride refusals."""
    import tensorfs

    observations = []
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
            value = checkpoint.components["weights"].tensors.read(row["key"])
            assert tuple(value.shape) == tuple(row["shape"])
            assert value.tobytes() == np.ones(row["shape"], dtype="<f4").tobytes()
        checkpoint.close()
    return Report(True, json.dumps({"arrays": observations, "nonempty_strided_refused": True,
                                   "tensorfs": tensorfs.__version__}, sort_keys=True))


class ComponentsConverted(msgspec.Struct):
    passed: bool
    model: ModelArtifact
    rowwise: ModelArtifact
    detail: str


@app.entrypoint(
    accelerator=False,
    weights=(WeightsOutput("components", 2 << 20), WeightsOutput("rowwise", 2 << 20)),
)
def runtime_components(
    payload: Request, ctx: Context, tel: Telemetry
) -> ComponentsConverted:
    """Two real component calls share full identity and publish only their own scope."""
    import tensorfs
    from cozy_runtime.author import CapabilityError, UnsupportedInput
    from cozy_runtime.derive import native_quantization
    from cozy_runtime.derive.quantization import (
        ArtifactQuantizationPlan,
        ArtifactQuantizationRequest,
        QuantizationTensor,
        quantize_component_into,
    )
    from cozy_runtime.derive.svdq import Calibration
    from cozy_runtime.weights_store import Derivation, Output

    rng = np.random.default_rng(payload.seed)
    with tempfile.TemporaryDirectory(prefix="cozy-svdq-components-") as temporary:
        directory = Path(temporary)
        writer = tensorfs.Writer(directory / "source.cozytensors")
        modules, values = {}, {}
        for name in ("a", "b"):
            weight = torch.from_numpy(
                rng.normal(0, 0.0625, (64, 256)).astype("<f4")
            ).bfloat16()
            modules[name] = torch.nn.Sequential(
                OrderedDict(
                    [
                        (
                            "proj",
                            torch.nn.Linear(256, 64, bias=False, dtype=torch.bfloat16),
                        )
                    ]
                )
            )
            with torch.no_grad():
                modules[name].proj.weight.copy_(weight)
            writer.component(name).put(
                "proj.weight",
                weight.view(torch.uint8).numpy().tobytes(),
                dtype="BF16",
                shape=[64, 256],
            )
            values[name] = weight.float().numpy()
        source = writer.commit()
        plan = ArtifactQuantizationPlan(
            ["a", "b"],
            [(name, "proj.weight") for name in ("a", "b")],
            [
                QuantizationTensor(name, "proj.weight", "bf16", (64, 256))
                for name in ("a", "b")
            ],
        )

        def collect() -> Calibration:
            calibration = Calibration(
                torch, modules, ("a.proj.weight", "b.proj.weight")
            )
            with calibration, torch.inference_mode():
                for _ in range(2):
                    for module in modules.values():
                        inputs = torch.from_numpy(
                            rng.normal(0, 0.125, (16, 256)).astype("<f4")
                        ).bfloat16()
                        module(inputs)
            return calibration

        calibration = collect()
        rows = native_quantization.resolved(
            source, plan, "svdq.nvfp4", lambda row: row.logical_dtype
        )
        request = ArtifactQuantizationRequest(fit_workspace_bytes=WORKSPACE)
        with native_quantization.snapshot(
            source, plan, "svdq.nvfp4", calibration, ctx
        ) as snapshot:
            assert snapshot is not None
            work = Derivation(
                base=source,
                sources=(snapshot,),
                order=plan.order,
                provenance=native_quantization.provenance(source, rows, snapshot),
            )
            with ctx.output("components", work) as output:
                assert not output.has("a", "proj.weight") and not output.has(
                    "b", "proj.weight"
                )
                try:
                    quantize_component_into(
                        source,
                        output,
                        ctx,
                        request,
                        tel,
                        encoding="svdq.nvfp4",
                        plan=plan,
                        component="missing",
                        logical_dtype=lambda row: row.logical_dtype,
                        calibration=calibration,
                    )
                except UnsupportedInput:
                    assert output.written == 0
                else:
                    raise AssertionError(
                        "unknown component did not refuse before writes"
                    )
                made = []
                for name in ("a", "b"):
                    measured = quantize_component_into(
                        source,
                        output,
                        ctx,
                        request,
                        tel,
                        encoding="svdq.nvfp4",
                        plan=plan,
                        component=name,
                        logical_dtype=lambda row: row.logical_dtype,
                        calibration=calibration,
                    )
                    assert measured.encoded_keys == 1 and measured.reused_keys == 0
                    assert measured.source_bytes_read == 64 * 256 * 2
                    assert measured.new_bytes_written == encoded_bytes((64, 256))
                    assert (
                        len(measured.tensors) == 1
                        and measured.tensors[0]["component"] == name
                    )
                    assert output.has(name, "proj.weight")
                    if name == "a":
                        assert not output.has("b", "proj.weight"), (
                            "A call wrote future component B"
                        )
                        assert output.written == encoded_bytes((64, 256))
                    made.append(measured)
                assert output.written == 2 * encoded_bytes((64, 256))
                artifact = ctx.adopt_model(output)
                store, slot, transaction = output.store, output.slot, output.transaction
            # Actual finished Output reuse must validate the same complete plan and scope.
            with Output(store, slot, transaction, work, 2 << 20) as reopened:
                assert reopened.done is not None
                for name in ("a", "b"):
                    reused = quantize_component_into(
                        source,
                        reopened,
                        ctx,
                        request,
                        tel,
                        encoding="svdq.nvfp4",
                        plan=plan,
                        component=name,
                        logical_dtype=lambda row: row.logical_dtype,
                        calibration=calibration,
                    )
                    assert reused.encoded_keys == 0 and reused.reused_keys == 1
                    assert (
                        reused.source_bytes_read == 0 and reused.new_bytes_written == 0
                    )
                    assert reused.worst_relative_frobenius is None
                changed = collect()
                try:
                    quantize_component_into(
                        source,
                        reopened,
                        ctx,
                        request,
                        tel,
                        encoding="svdq.nvfp4",
                        plan=plan,
                        component="a",
                        logical_dtype=lambda row: row.logical_dtype,
                        calibration=changed,
                    )
                except CapabilityError as error:
                    assert error.code == "quantization_identity"
                    changed_refusal = str(error)
                else:
                    raise AssertionError(
                        "finished Output reused a different calibration"
                    )
        # Noncalibrated native outputs bind the same complete producer identity.
        plain_rows = native_quantization.resolved(
            source, plan, "fp8.rowwise", lambda row: row.logical_dtype
        )
        plain_work = Derivation(
            base=source,
            order=plan.order,
            provenance=native_quantization.provenance(source, plain_rows, None),
        )
        with ctx.output("rowwise", plain_work) as plain_output:
            for name in ("a", "b"):
                plain = quantize_component_into(
                    source,
                    plain_output,
                    ctx,
                    request,
                    tel,
                    encoding="fp8.rowwise",
                    plan=plan,
                    component=name,
                    logical_dtype=lambda row: row.logical_dtype,
                )
                assert plain.encoded_keys == 1 and plain.reused_keys == 0
                assert plain.source_bytes_read == 64 * 256 * 2
                assert plain.new_bytes_written == 64 * (256 + 4)
                assert len(plain.tensors) == 1 and plain.tensors[0]["component"] == name
                if name == "a":
                    assert not plain_output.has("b", "proj.weight")
            rowwise = ctx.adopt_model(plain_output)
            plain_store, plain_slot, plain_transaction = (
                plain_output.store,
                plain_output.slot,
                plain_output.transaction,
            )
        nonfit_refusals = []
        with Output(
            plain_store, plain_slot, plain_transaction, plain_work, 2 << 20
        ) as reopened:
            for name in ("a", "b"):
                again = quantize_component_into(
                    source,
                    reopened,
                    ctx,
                    request,
                    tel,
                    encoding="fp8.rowwise",
                    plan=plan,
                    component=name,
                    logical_dtype=lambda row: row.logical_dtype,
                )
                assert again.encoded_keys == 0 and again.reused_keys == 1
                assert again.source_bytes_read == 0 and again.new_bytes_written == 0
                assert again.worst_relative_frobenius is None
            reduced = ArtifactQuantizationPlan(
                ["a"], [("a", "proj.weight")], plan.tensors[:1]
            )
            for changed_plan, changed_encoding in (
                (plan, "int8.convrot"),
                (reduced, "fp8.rowwise"),
            ):
                try:
                    quantize_component_into(
                        source,
                        reopened,
                        ctx,
                        request,
                        tel,
                        encoding=changed_encoding,
                        plan=changed_plan,
                        component="a",
                        logical_dtype=lambda row: row.logical_dtype,
                    )
                except CapabilityError as error:
                    assert error.code == "quantization_identity"
                    nonfit_refusals.append(str(error))
                else:
                    raise AssertionError(
                        "public helper reused changed nonfit native work"
                    )
        committed = tensorfs.open(artifact.checkpoint, local_files_only=True)
        try:
            facts = {}
            for name, measured in zip(("a", "b"), made, strict=True):
                item = committed.components[name].tensors.read("proj.weight")
                facts[name] = check_selected(
                    item,
                    values[name],
                    measured.tensors[0],
                    calibration=calibration.stats(name, "proj.weight")
                    .rows.float()
                    .numpy(),
                )
            assert committed.verify().ok
        finally:
            committed.close()
            source.close()
    return ComponentsConverted(
        True,
        artifact,
        rowwise,
        json.dumps(
            {
                "components": facts,
                "different_finished_calibration_refused": changed_refusal,
                "nonfit_identity_refusals": nonfit_refusals,
                "full_plan_single_identity": True,
                "future_component_not_written": True,
            },
            sort_keys=True,
        ),
    )
