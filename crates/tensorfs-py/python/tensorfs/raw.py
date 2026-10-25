"""Physical format inspection, native tensor encoding and resident decoding."""

from ._ext import raw as _raw

Part = _raw.Part
parts = _raw.parts
decode_into = _raw.decode_into
quantize = _raw.quantize

__all__ = ["Part", "decode_into", "parts", "quantize"]
