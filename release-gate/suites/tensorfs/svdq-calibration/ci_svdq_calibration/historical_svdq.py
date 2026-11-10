"""Frozen Torch SVDQ producer reference. No current production fitter or decoder import.

Original fitting source and provenance are retained beside this file and in ORACLE.md.
Only import plumbing is adapted: NVFP4 encoding and fake/decode laws are frozen here.
"""

from __future__ import annotations

import time
import zlib
from types import ModuleType, SimpleNamespace
from typing import TYPE_CHECKING, NamedTuple, cast

import numpy as np

from .oracle import E2M1_DECODE32, NVFP4_BLOCK, Encoded, encode_nvfp4

if TYPE_CHECKING:
    import numpy.typing as npt
    from torch import Tensor
    from torch import device as Device
    from torch import dtype as DType

    from ._types import Calibrated, SvdqLinear

ALPHAS = (0.0, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8)
RANK, BLOCK, MARGIN = 32, 2048, 2.0


class Stats(NamedTuple):
    xmax: Tensor
    rows: Tensor


def _fake(torch: ModuleType, xs: Tensor, g: Tensor) -> Tensor:
    blocks = xs.view(xs.shape[0], -1, 16)
    amax = blocks.abs().amax(-1)
    scale = (amax / (6.0 * g)).clamp(2.0**-9, 448.0).to(torch.float8_e4m3fn)
    a = (blocks / (scale.float() * g).unsqueeze(-1)).abs()
    code = cast("Tensor", sum(  # cast: seven Tensor terms, never the empty sum's 0
        ((a >= t) if odd else (a > t)).to(torch.uint8)
        for t, odd in (
            (0.25, 0),
            (0.75, 1),
            (1.25, 0),
            (1.75, 1),
            (2.5, 0),
            (3.5, 1),
            (5.0, 0),
        )
    ))
    code = code + (blocks < 0).to(torch.uint8) * 8
    table: Tensor = torch.tensor(E2M1_DECODE32, dtype=torch.float32, device=xs.device)
    return (
        table[code.int()] * scale.float().unsqueeze(-1) * g.float().reshape(())
    ).view(xs.shape)


def nvfp4_values(torch: ModuleType, packed: Tensor, scale: Tensor, g: Tensor) -> Tensor:
    codes: Tensor = torch.stack((packed & 15, packed >> 4), dim=-1).reshape(packed.shape[0], -1)
    table: Tensor = torch.tensor(E2M1_DECODE32, dtype=torch.float32, device=packed.device)
    blocks = table[codes.int()].view(packed.shape[0], -1, 16)
    return (blocks * scale.float().unsqueeze(-1) * g.float().reshape(())).reshape(
        codes.shape
    )


svdq_linear = cast("SvdqLinear", SimpleNamespace(fake=_fake, LEVELS_MAX=2688.0))  # cast: the serving module's two names


class Fitted(NamedTuple):
    """One weight encoded: the residual's codes, each companion's raw bytes in `companions`
    order, the alpha that chose them, the residual's amax and nvfp4 round-trip error (what the
    encoder's bound holds), the logical weight's squared error and energy, and the mean per-token
    output error on the reservoir of the scheme and of plain nvfp4 W4A4."""

    payload: bytes
    companions: tuple[bytes, ...]
    alpha: float
    amax: float
    max_abs_err: float
    sse: float
    energy: float
    saturated: int
    error: float
    nvfp4_error: float
    #: Seconds the residual's nvfp4 encode took (on the host).
    encode_s: float


def companions(rows: int, cols: int) -> tuple[tuple[str, str, tuple[int, ...]], ...]:
    """The parts after `weight`, `(role, dtype, shape)`, in the order `fit` writes them."""
    return (
        ("scale", "F8_E4M3", (rows, cols // NVFP4_BLOCK)),
        ("scale_2", "F32", ()),
        ("pre_quant_scale", "F32", (cols,)),
        ("lowrank_down", "BF16", (RANK, cols)),
        ("lowrank_up", "BF16", (rows, RANK)),
        ("input_scale", "F32", ()),
    )


def _bf16(torch: ModuleType, t: Tensor) -> Tensor:
    return t.to(torch.bfloat16).float()


def fit(
    torch: ModuleType, key: str, weight: npt.NDArray[np.float32], stats: Calibrated, device: str | Device
) -> Fitted:
    """`svdq.nvfp4` for one weight: the alpha whose whole scheme best reproduces the reservoir's
    outputs, token by token; then the stored parts. A weight's temporaries are bounded by
    `BLOCK` rows of it at a time beside the weight and its residual."""
    w: Tensor = torch.from_numpy(np.ascontiguousarray(weight, dtype=np.float32)).to(device)
    rows = stats.rows.to(device).float()
    if len(rows) == 0:
        raise ValueError(f"{key}: calibration kept no rows")
    xmax = stats.xmax.to(device).float().clamp(min=1e-5)
    ref = rows @ w.t()

    def off(y: Tensor) -> float:
        return float(((y - ref).norm(dim=1) / ref.norm(dim=1).clamp(min=1e-30)).mean())

    def scale(t: Tensor) -> Tensor:
        return (t.abs().amax() / svdq_linear.LEVELS_MAX).clamp(min=1e-30)

    plain = off(
        _through(torch, svdq_linear.fake(torch, _bf16(torch, rows), scale(rows)), w)
    )
    wmax = w.abs().amax(0).clamp(min=1e-5)
    best: tuple[float, float, Tensor, Tensor, Tensor] | None = None
    for alpha in ALPHAS:
        s: Tensor = (
            torch.ones_like(xmax)
            if alpha == 0
            else (xmax**alpha / wmax ** (1 - alpha)).clamp(1e-5)
        )
        s = s / s.mean()
        u, sv, vt = _top(torch, w * s, RANK + 16, zlib.crc32(key.encode()))
        up, down = _bf16(torch, u[:, :RANK] * sv[:RANK]), _bf16(torch, vt[:RANK] / s)
        residual = w * s - up @ (down * s)
        gx = ((xmax / s).amax() * MARGIN / svdq_linear.LEVELS_MAX).clamp(min=1e-30)
        y = _through(
            torch, svdq_linear.fake(torch, _bf16(torch, rows / s), gx), residual
        )
        err = off(y + _bf16(torch, rows @ down.t()) @ up.t())
        if best is None or err < best[0]:
            best = (err, alpha, s, up, down)
        del residual, y
    assert best is not None
    _, alpha, s, up, down = best
    residual = w * s - up @ (down * s)
    started = time.perf_counter()
    encoded = encode_nvfp4(key, residual.cpu().numpy())
    encode_s = time.perf_counter() - started
    gx = (xmax / s).amax() * MARGIN / svdq_linear.LEVELS_MAX
    p = 1.0 / s
    stored = _decode(torch, encoded, residual.shape, device)
    y = svdq_linear.fake(torch, _bf16(torch, rows * p), gx) @ stored.t()
    served = off(y + _bf16(torch, rows @ down.t()) @ up.t())
    amax, round_trip = (
        float(residual.abs().max()),
        float((stored - residual).abs().max()),
    )
    del residual
    error = stored.mul_(p).add_(up @ down).sub_(w)
    scales, second = encoded.companions

    def raw(t: Tensor, dtype: DType) -> bytes:
        return bytes(
            t.to(dtype)
            .reshape(-1)
            .contiguous()
            .cpu()
            .view(torch.uint8)
            .numpy()
            .tobytes()
        )

    return Fitted(
        encoded.payload,
        (
            scales.raw,
            second.raw,
            raw(p, torch.float32),
            raw(down, torch.bfloat16),
            raw(up, torch.bfloat16),
            raw(gx.reshape(()), torch.float32),
        ),
        alpha,
        amax,
        round_trip,
        float(error.square().sum(dtype=torch.float64)),
        float(w.square().sum(dtype=torch.float64)),
        encoded.saturated,
        served,
        plain,
        encode_s,
    )


def _through(torch: ModuleType, xq: Tensor, weight: Tensor) -> Tensor:
    """`xq @ fake(weight)ᵀ`: `weight` through nvfp4 at its own second level, `BLOCK` rows at a
    time."""
    g = (weight.abs().amax() / svdq_linear.LEVELS_MAX).clamp(min=1e-30)
    return cast("Tensor", torch.cat(  # cast: `torch` is handed in as a module
        [xq @ svdq_linear.fake(torch, part, g).t() for part in weight.split(BLOCK)],
        dim=1,
    ))


def _top(torch: ModuleType, a: Tensor, q: int, seed: int) -> tuple[Tensor, Tensor, Tensor]:
    """`a`'s top `q` singular triplets `(U, S, Vᵀ)` by randomized subspace iteration from a
    `seed`ed start: the same weight fits to the same bytes on every run."""
    start = torch.Generator(device=a.device).manual_seed(seed)
    y = a @ torch.randn(a.shape[1], q, generator=start, device=a.device)
    for _ in range(4):
        y = a @ (a.t() @ torch.linalg.qr(y).Q)
    basis = torch.linalg.qr(y).Q
    u, sv, vt = torch.linalg.svd(basis.t() @ a, full_matrices=False)
    return basis @ u, sv, vt


def _decode(torch: ModuleType, encoded: Encoded, shape: tuple[int, ...], device: str | Device) -> Tensor:
    """`encode_nvfp4`'s payload back to f32, by the TensorFS law."""
    rows, cols = int(shape[0]), int(shape[1])
    weight = torch.frombuffer(bytearray(encoded.payload), dtype=torch.uint8).view(
        rows, cols // 2
    )
    scale, second = encoded.companions
    block = torch.frombuffer(bytearray(scale.raw), dtype=torch.uint8).view(
        torch.float8_e4m3fn
    )
    g = torch.frombuffer(bytearray(second.raw), dtype=torch.float32)
    return nvfp4_values(
        torch,
        weight.to(device),
        block.view(rows, cols // NVFP4_BLOCK).to(device),
        g.to(device),
    )
