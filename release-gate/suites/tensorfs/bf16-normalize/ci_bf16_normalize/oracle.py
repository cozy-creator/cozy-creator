"""Frozen Runtime BF16 carrier conversion and independent stored-source reference.

Carrier conversion definitions copied verbatim from Runtime9bf8b6b1f.
Normalization statistics preserve write_bf16's operation order; all stored reads
and arithmetic below are independent of the native normalization implementation.
No imports of current production converters or model-value decoders.
"""
from __future__ import annotations
import math
import numpy as np
from cozy_runtime.author import UnsupportedInput

# fmt: off
def numel(shape: list[int]) -> int:
    total = 1
    for d in shape:
        total *= int(d)
    return total

def to_f32(raw: bytes | bytearray | memoryview, dtype: str, shape: list[int]) -> np.ndarray:
    """Decode a FLOAT tensor's stored bytes to float32. Refuses anything else, loudly:
    a quantizer that silently reinterprets an int tensor as floats produces plausible
    numbers from bytes that never were numbers."""
    count = numel(shape)
    if dtype == "F32":
        out = np.frombuffer(raw, dtype="<f4", count=count)
    elif dtype == "F16":
        out = np.frombuffer(raw, dtype="<f2", count=count).astype(np.float32)
    elif dtype == "BF16":
        # bf16 is the high half of an f32: widen by placing the 16 bits in the top half.
        wide = np.zeros(count * 2, dtype=np.uint16)
        wide[1::2] = np.frombuffer(raw, dtype="<u2", count=count)
        out = wide.view("<f4")
    else:
        raise UnsupportedInput(
            f"{dtype} is not a float carrier this quantizer reads — it quantizes float "
            "weights, and reinterpreting other bytes as floats is how a plausible wrong "
            "artifact gets made"
        )
    return np.ascontiguousarray(out, dtype=np.float32).reshape(shape)

def from_f32(values: np.ndarray, dtype: str) -> bytes:
    """Encode float32 back to a stored float carrier, round-to-nearest-EVEN throughout."""
    flat = np.ascontiguousarray(values, dtype=np.float32).reshape(-1)
    if dtype == "F32":
        return flat.tobytes()
    if dtype == "F16":
        return flat.astype("<f2").tobytes()
    if dtype == "BF16":
        bits = flat.view(np.uint32)
        # RNE on the truncated 16 low bits: add the rounding bias then take the top half.
        rounded = (bits + 0x7FFF + ((bits >> 16) & 1)) >> 16
        return rounded.astype("<u2").tobytes()
    raise UnsupportedInput(f"{dtype} is not a float carrier this writer emits")


def normalize(values: np.ndarray) -> tuple[bytes, dict[str, float | int]]:
    """write_bf16's exact carrier conversion and operation order, on a fixture window.

    Empty dimensions are deliberate native coverage; the old _read_chunks loop had
    no rows to read for a leading empty dimension. No source decoder is imported.
    """
    values = np.asarray(values, dtype=np.float32)
    if not np.isfinite(values).all():
        raise ValueError("nonfinite source")
    raw = from_f32(values, "BF16")
    reconstructed = to_f32(raw, "BF16", list(values.shape))
    if not np.isfinite(reconstructed).all():
        raise ValueError("BF16 overflow")
    error = np.abs(values - reconstructed)
    amax = float(np.max(np.abs(values))) if values.size else 0.0
    max_error = float(np.max(error)) if values.size else 0.0
    sse = float(np.sum(error.astype(np.float64) ** 2))
    energy = float(np.sum(values.astype(np.float64) ** 2))
    if max_error > amax * 2.0**-8:
        raise ValueError("BF16 round-trip quality refusal")
    return raw, {
        "amax": amax,
        "max_abs_error": max_error,
        "sse": sse,
        "energy": energy,
        "saturated": 0,
        "relative_frobenius": math.sqrt(sse / energy) if energy else 0.0,
    }


def element(code: int, dtype: str) -> float:
    """Independent exact finite FP8/I8 element law, then the historical F32 multiply."""
    if dtype == "I8":
        return float(code - 256 if code >= 128 else code)
    if dtype == "F8_E4M3":
        exponent, mantissa = (code >> 3) & 15, code & 7
        if exponent == 15 and mantissa == 7:
            return float("nan")
        magnitude = mantissa * 2.0**-9 if exponent == 0 else (8 + mantissa) * 2.0**(exponent - 10)
    elif dtype == "F8_E5M2":
        exponent, mantissa = (code >> 2) & 31, code & 3
        if exponent == 31:
            return float("inf") if mantissa == 0 else float("nan")
        magnitude = mantissa * 2.0**-16 if exponent == 0 else (4 + mantissa) * 2.0**(exponent - 17)
    else:
        raise ValueError(dtype)
    return -magnitude if code & 128 else magnitude


def rowwise(codes: bytes, weight_dtype: str, scales: bytes, scale_dtype: str,
            shape: tuple[int, int]) -> np.ndarray:
    """Runtime9bf8b6b1f RowwiseDequant: widen payload+scale, multiply once in F32.

    Logical source dtype is intentionally absent: it is metadata, not another
    narrowing boundary before BF16 normalization.
    """
    table = np.array([element(code, weight_dtype) for code in range(256)], dtype=np.float32)
    payload = table[np.frombuffer(codes, dtype=np.uint8)].reshape(shape)
    scale = to_f32(scales, scale_dtype, [shape[0]])
    return payload * scale.reshape(-1, 1)
