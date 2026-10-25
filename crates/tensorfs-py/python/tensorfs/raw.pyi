"""Physical format operations; ordinary reads use Component.tensors."""

from collections.abc import Mapping, Sequence

from typing_extensions import Buffer

from ._ext import Checkpoint, Encoded
from .api import DLPack

class Part:
    """One stored part: a plain tensor's data (role `""`) or an encoded tensor's part."""

    @property
    def dtype(self) -> str: ...
    @property
    def shape(self) -> tuple[int, ...]: ...
    @property
    def chunk(self) -> int: ...
    @property
    def offset(self) -> int: ...
    @property
    def length(self) -> int: ...

def parts(checkpoint: Checkpoint, component: str, key: str) -> dict[str, Part]:
    """Stored ranges only; does not fetch or load array bytes."""

def quantize(
    scheme: str,
    values: bytes,
    *,
    dtype: str,
    shape: Sequence[int],
    strategy: str,
    params: Mapping[str, object] | None = None,
    tensor_amax: float | None = None,
) -> Encoded:
    """Encode immutable little-endian F32 values with an explicit producer revision.

    Admits int8.convrot / int8-convrot256/1 with rotation_size=256 and
    nvfp4 / nvfp4-min-sse/1 in low-first, row-major, multiply layout.
    NVFP4 requires the finite nonnegative whole-tensor tensor_amax on every chunk;
    ConvRot refuses that argument. Global-scale underflow/overflow is refused.
    Complete rows may be chunked without changing output bytes. The native arithmetic
    releases the GIL and uses one row (ConvRot) or block (NVFP4) of floating scratch,
    plus the owned output.
    This encodes supplied tensor bytes; it does not run a model or write a checkpoint.
    """

def decode_into(
    scheme: str,
    parts: Mapping[str, tuple[str, Sequence[int], Buffer | DLPack]],
    out: Buffer | DLPack,
    *,
    dtype: str,
    shape: Sequence[int],
    params: Mapping[str, object] | None = None,
) -> None:
    """Decode existing contiguous part storage into an exact logical destination.

    Part tuples describe format bytes; carriers may be raw byte views. The output
    must match logical dtype and shape, without casting or input/output aliasing.
    CUDA parts and output must be on one device; mixed placement is unsupported.
    """
