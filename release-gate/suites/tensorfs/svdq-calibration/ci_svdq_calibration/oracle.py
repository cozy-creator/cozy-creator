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

def _e4m3_magnitudes() -> np.ndarray:
    """Byte 0x00..0x7E in ascending magnitude; 0x7F is NaN and is not representable here.
    exp==0 is the subnormal run man*2^-9; otherwise (1+man/8)*2^(exp-7). Max 448."""
    mags = np.empty(127, dtype=np.float64)
    for b in range(127):
        exp, man = (b >> 3) & 0x0F, b & 0x07
        mags[b] = man * 2.0**-9 if exp == 0 else (1.0 + man / 8.0) * 2.0 ** (exp - 7)
    return mags

E4M3_MAG = _e4m3_magnitudes()

E4M3_MAG32 = E4M3_MAG.astype(np.float32)

E4M3_MAX = float(E4M3_MAG[-1])

E2M1_MAG = np.array([0.0, 0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0], dtype=np.float64)

E2M1_MAG32 = E2M1_MAG.astype(np.float32)

E2M1_MAX = 6.0

E2M1_DECODE32 = np.concatenate([E2M1_MAG32, -E2M1_MAG32]).astype(np.float32)

def _nearest_even(mags: np.ndarray, x: np.ndarray) -> np.ndarray:
    """Index of the nearest table magnitude, ties to the EVEN index.

    Even index is even mantissa-LSB for both tables (E2M1: 0, 1.0, 2.0, 4.0 have man 0;
    E4M3: byte parity IS the mantissa LSB), so this is round-to-nearest-even in the
    representation, not merely in the index.
    """
    mids = (mags[:-1] + mags[1:]) / 2.0
    idx = np.searchsorted(mids, x, side="right")
    tie = (idx > 0) & (x == mids[np.maximum(idx - 1, 0)])
    return np.where(tie & (idx % 2 == 1), idx - 1, idx).astype(np.uint8)

def e2m1_encode(values: np.ndarray) -> np.ndarray:
    """float32 -> E2M1 nibbles (0..15), RNE, saturating at +/-6."""
    v = np.nan_to_num(
        np.asarray(values, dtype=np.float32), nan=0.0, posinf=E2M1_MAX, neginf=-E2M1_MAX
    )
    mag = np.minimum(np.abs(v).astype(np.float64), E2M1_MAX)
    idx = _nearest_even(E2M1_MAG, mag)
    out: np.ndarray = (idx | np.where(np.signbit(v), 0x08, 0x00).astype(np.uint8)).astype(np.uint8)
    return out

def pack_nibbles(nibbles: np.ndarray) -> np.ndarray:
    """[r, c] nibbles -> [r, c/2] bytes, element 2k in the LOW nibble (the border's
    packing; `nvfp4:nibbles-swapped` is one of its own WRONG conformance arms)."""
    rows, cols = nibbles.shape
    pairs = nibbles.reshape(rows, cols // 2, 2)
    out: np.ndarray = (pairs[:, :, 0] | (pairs[:, :, 1] << 4)).astype(np.uint8)
    return out

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

NVFP4_BLOCK = 16

def encode_nvfp4(key: str, values: np.ndarray, amax: float | None = None) -> Encoded:
    """The two-level NVFP4 scheme, in the border's `nvfp4-w4a4/1` base-variant layout.

    REFERENCE. The scheme is NVIDIA's NVFP4 as exported by TensorRT Model Optimizer and
    decoded by this platform's own border: `value[i,j] = e2m1(nibble) *
    e4m3(weight_scale[i, j/16]) * weight_scale_2`, with `weight` packed u8 [out, in/2],
    `weight_scale` e4m3 [out, in/16] and `weight_scale_2` an f32 SCALAR. That decode line
    is the contract this producer is written against (registry.rs `nvfp4-w4a4/1`,
    conform.rs `nvfp4_inner`) — a second-hand description of the format is not, which
    matters because two of the border's three conformance arms for this encoding are
    plausible misreadings (`nibbles-swapped`, `global-scale-ignored`).

    The second-level scalar is `amax / (448 * 6)`: it maps the tensor's largest element to
    the largest product the two levels can express, so the block requiring the most range
    lands at E4M3's own maximum and no block requirement is unrepresentable. Per block the
    exact requirement is `block_amax / (6 * s2)` and the scale is chosen by min-SSE
    through the stored representation over the family rule's candidate set — the
    rounded-down (CLIPPING) neighbour included, per #543d (see the module docstring).
    """
    rows, cols = _require_2d(key, values, NVFP4_BLOCK)
    nb = cols // NVFP4_BLOCK
    if amax is None:  # a whole tensor; a row chunk is handed its tensor's amax
        amax = float(np.max(np.abs(values))) if values.size else 0.0
    if not np.isfinite(amax):
        raise UnsupportedInput(f"{key!r} contains a non-finite weight — refuse, never encode")
    # An all-zero tensor has no scale to derive; 1.0 reproduces it exactly and keeps the
    # scalar a real number rather than a zero that makes every later division undefined.
    s2 = np.float32(amax / (E4M3_MAX * E2M1_MAX)) if amax > 0 else np.float32(1.0)

    payload = np.empty((rows, cols // 2), dtype=np.uint8)
    scales = np.empty((rows, nb), dtype=np.uint8)
    acc = _Accum()
    for sl in row_chunks(rows, cols):
        blk = np.ascontiguousarray(values[sl], dtype=np.float32).reshape(-1, nb, NVFP4_BLOCK)
        bamax = np.max(np.abs(blk), axis=-1)
        need = (bamax / np.float32(E2M1_MAX * s2)).astype(np.float64)
        # Saturation-free by construction: the smallest E4M3 value at or above `need`.
        idx = np.clip(np.searchsorted(E4M3_MAG, need, side="left"), 0, 126).astype(np.int64)
        # ...then by ARITHMETIC: re-check in the same float32 the encoder will use, and
        # bump anything the double-to-single narrowing left one ulp short. Two rounds is
        # a bound, not a hope — one bump moves the scale a full E4M3 step.
        for _ in range(3):
            eff = (E4M3_MAG32[idx] * s2).astype(np.float32)
            short = (bamax > eff * np.float32(E2M1_MAX)) & (idx < 126)
            if not bool(short.any()):
                break
            idx = np.where(short, idx + 1, idx)

        best_sse: np.ndarray | None = None
        best: tuple[np.ndarray, np.ndarray, np.ndarray] | None = None
        # THE FAMILY RULE (#543d, realigning this kind to #517): clipping candidates are
        # ADMISSIBLE. The rounded-DOWN neighbour clips the block's own largest element,
        # and in fp4 that trade WINS 0.33 dB weight-SNR on the banked audit — an E4M3
        # scale step is ~6% while the resolution gain applies to all 16 elements.
        # Iteration order is smallest scale first, so the strict `<` keeps the smaller
        # scale on ties; saturation is a REPORTED stat below, never a constraint.
        for cand in (np.maximum(idx - 1, 0), idx, np.minimum(idx + 1, 126)):
            eff = (E4M3_MAG32[cand] * s2).astype(np.float32)[..., None]
            safe = np.where(eff > 0, eff, np.float32(1.0))
            nib = e2m1_encode(np.where(eff > 0, blk / safe, np.float32(0.0)))
            recon = (E2M1_DECODE32[nib] * eff).astype(np.float32)
            sse = np.sum((recon - blk).astype(np.float64) ** 2, axis=-1)
            if best_sse is None:
                best_sse, best = sse, (cand, nib, recon)
            else:
                assert best is not None
                take = sse < best_sse  # strict: a tie keeps the SMALLER scale
                best_sse = np.where(take, sse, best_sse)
                best = (
                    np.where(take, cand, best[0]),
                    np.where(take[..., None], nib, best[1]),
                    np.where(take[..., None], recon, best[2]),
                )
        assert best is not None
        chosen, nib, recon = best
        limit = (E4M3_MAG32[chosen] * s2).astype(np.float32)[..., None] * np.float32(E2M1_MAX)
        acc.sat += int(np.count_nonzero(np.abs(blk) > limit))
        acc.add(blk.reshape(-1, cols), recon.reshape(-1, cols))
        payload[sl] = pack_nibbles(nib.reshape(-1, cols))
        scales[sl] = chosen.astype(np.uint8)

    return Encoded(
        payload_dtype="U8",
        payload_shape=[rows, cols // 2],
        payload=payload.tobytes(),
        companions=[
            Companion(".weight_scale", "F8_E4M3", [rows, nb], scales.tobytes()),
            Companion(".weight_scale_2", "F32", [], np.float32(s2).tobytes()),
        ],
        saturated=acc.sat,
        sse=acc.sse,
        energy=acc.energy,
        max_abs_err=acc.max_abs,
    )
# fmt: on
