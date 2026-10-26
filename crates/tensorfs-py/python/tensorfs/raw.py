"""Physical format inspection, native tensor encoding and resident decoding."""

from typing import TypedDict

from ._ext import raw as _raw


class QuantizationStats(TypedDict):
    """Native acceptance facts over the supplied F32 values, before logical rounding."""

    amax: float
    max_abs_error: float
    sse: float
    energy: float
    saturated: int


Part = _raw.Part
parts = _raw.parts
decode_into = _raw.decode_into
quantize = _raw.quantize
quantize_measured = _raw.quantize_measured
quantization_implementation = _raw.quantization_implementation
normalization_implementation = _raw.normalization_implementation

__all__ = [
    "Part",
    "QuantizationStats",
    "decode_into",
    "parts",
    "quantize",
    "quantize_measured",
    "quantization_implementation",
    "normalization_implementation",
]
