"""Historical numerical oracle for this standalone real release-gate package.

Selected definitions copied verbatim from cozy-runtime1c092cd8474f1a3d042d9e1ee69d61410e1adb04
microscale.py and safetensors_io.row_chunks. This must not import the current production encoders:
after native cutover that would compare Rust against itself. See ORACLE.md beside this package.
"""

from __future__ import annotations

from collections.abc import Iterator
from typing import NamedTuple

import numpy as np
from cozy_runtime.author import UnsupportedInput


# fmt: off
def row_chunks(rows: int, cols: int, budget_elements: int = 1 << 22) -> Iterator[slice]:
    """Row slices whose working set stays inside `budget_elements`. Every encoder in this
    family is row-independent, so chunking changes no number — it only decides whether a
    [28672, 5376] tensor needs 600 MB of float32 scratch or 16 MB of it."""
    per = max(1, budget_elements // max(1, cols))
    for start in range(0, rows, per):
        yield slice(start, min(start + per, rows))

CACHE_ELEMENTS = 1 << 18

class Companion(NamedTuple):
    """One scale sibling: the carrier SUFFIX (appended to the module STEM, #501d), the
    header dtype, the shape, and the stored bytes."""

    suffix: str
    dtype: str
    shape: list[int]
    raw: bytes

class Encoded(NamedTuple):
    payload_dtype: str
    payload_shape: list[int]
    payload: bytes
    companions: list[Companion]
    saturated: int
    """Elements whose magnitude exceeded the payload format's range AFTER scaling. The
    #517 defect class reduced to one integer, carried into the manifest."""
    sse: float
    energy: float
    max_abs_err: float

    def relative_frobenius(self) -> float:
        """||W - dequant(quant(W))||_F / ||W||_F — the per-tensor quality number this
        family banks. Zero energy (an all-zero tensor) is exactly reproduced, so 0.0."""
        return float(np.sqrt(self.sse / self.energy)) if self.energy > 0 else 0.0

class _Accum:
    __slots__ = ("energy", "max_abs", "sat", "sse")

    def __init__(self) -> None:
        self.sse = 0.0
        self.energy = 0.0
        self.max_abs = 0.0
        self.sat = 0

    def add(self, source: np.ndarray, recon: np.ndarray) -> None:
        wide = source.astype(np.float64)
        diff = recon.astype(np.float64)
        diff -= wide
        if diff.size:
            self.max_abs = max(self.max_abs, float(diff.max()), -float(diff.min()))
        self.sse += float(np.sum(np.square(diff, out=diff)))
        self.energy += float(np.sum(np.square(wide, out=wide)))

def _require_2d(key: str, values: np.ndarray, block: int) -> tuple[int, int]:
    if values.ndim != 2:
        raise UnsupportedInput(
            f"{key!r} is rank {values.ndim}: this family encodes 2-D GEMM weights, and a "
            "norm or a bias stays at source precision rather than being reshaped into one"
        )
    rows, cols = int(values.shape[0]), int(values.shape[1])
    if cols % block:
        raise UnsupportedInput(
            f"{key!r} has {cols} columns, not a multiple of the {block}-element block — "
            "the encoding's blocking is a shape fact and padding it would change the "
            "tensor the consumer reads"
        )
    return rows, cols

CONVROT_GROUP = 256

INT8_MAX = 127.0

def hadamard256(values: np.ndarray) -> np.ndarray:
    """Each 256-group of `[rows, K]` times the regular Hadamard H4⊗H4⊗H4⊗H4, entries ±1 and
    UNNORMALIZED (orthonormal is this / 16). H4 = [[1,1,1,-1],[1,1,-1,1],[1,-1,1,1],[-1,1,1,1]]
    is symmetric, so H is its own transpose and H·H = 256·I. Additions only: an integer input
    stays an exact integer, which is what makes the decode below exact."""
    rows, cols = values.shape
    work = values.reshape(-1, 4, 4, 4, 4)
    for axis in range(1, 5):
        a, b, c, d = (np.take(work, i, axis=axis) for i in range(4))
        work = np.stack((a + b + c - d, a + b - c + d, a - b + c + d, b + c + d - a), axis=axis)
    return work.reshape(rows, cols)

def decode_int8_convrot256(payload: np.ndarray, scale: np.ndarray) -> np.ndarray:
    """`int8-convrot256/1`'s value law, exactly: `scale[i] * (data[i] · H)[j] / 16` in f64.
    The Hadamard sum of int8 is an exact integer and every factor after it is exact in f64,
    so the only rounding is the caller's one cast into the logical dtype."""
    sums = hadamard256(payload.astype(np.int32)).astype(np.float64)
    return sums * scale.astype(np.float64)[:, None] / 16.0

def encode_int8_convrot256(key: str, values: np.ndarray) -> Encoded:
    """`int8-convrot256/1`: each row rotated by the orthonormal Hadamard per 256 input
    channels (ComfyUI's ConvRot), then symmetric per-output-row int8: `scale = rowmax|Wh|/127`
    (f32, floor 1e-12) and RNE `clip(rint(Wh / scale), ±127)`. The rotation spreads a row's
    outliers across its group, which is what keeps the per-token int8 activations that pair
    with it accurate; the leaf rotates each activation by the same H."""
    rows, cols = _require_2d(key, values, CONVROT_GROUP)
    payload = np.empty((rows, cols), dtype=np.int8)
    row_scale = np.empty((rows,), dtype=np.float32)
    acc = _Accum()
    for sl in row_chunks(rows, cols, CACHE_ELEMENTS):
        v = np.ascontiguousarray(values[sl], dtype=np.float32)
        rotated = hadamard256(v.astype(np.float64)) / 16.0
        amax = np.abs(rotated).max(axis=-1)
        scale = np.maximum(amax / INT8_MAX, 1e-12).astype(np.float32)
        q = np.rint(rotated / scale.astype(np.float64)[:, None])
        acc.sat += int(np.count_nonzero(np.abs(q) > INT8_MAX))
        q = np.clip(q, -INT8_MAX, INT8_MAX).astype(np.int8)
        acc.add(v, decode_int8_convrot256(q, scale))
        payload[sl] = q
        row_scale[sl] = scale
    return Encoded(
        payload_dtype="I8",
        payload_shape=[rows, cols],
        payload=payload.tobytes(),
        companions=[Companion(".weight_scale", "F32", [rows], row_scale.tobytes())],
        saturated=acc.sat,
        sse=acc.sse,
        energy=acc.energy,
        max_abs_err=acc.max_abs,
    )
# fmt: on
