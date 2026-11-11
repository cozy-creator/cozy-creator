"""The selected weight's independent checks: its stored parts against the residual the native fit selected, packed by
the frozen NVFP4 reference (`oracle.py`), and its stored value law against the logical weight. TensorFS's own fitting,
cancel and resume are the program `programs/svdq_native.py`."""

from __future__ import annotations

import hashlib
from typing import TYPE_CHECKING, cast

import numpy as np

from .oracle import E2M1_DECODE32, E4M3_MAG32, encode_nvfp4

if TYPE_CHECKING:
    from ._types import F32, ArrayTensor, Converted, EncodedTensor, Facts, Floats, Parts

STRATEGY = "svdq-nvfp4-r32-faer/1"
WORKSPACE = 64 << 20


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


def check_selected(
    item: EncodedTensor,
    weight: F32,
    measured: Converted,
    *,
    exact_svd: bool = True,
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
