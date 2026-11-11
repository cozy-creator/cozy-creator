"""Names for this package's annotations, imported by type checkers only."""

from collections.abc import Mapping, Sequence
from typing import TYPE_CHECKING, NotRequired, TypedDict

import numpy as np
import numpy.typing as npt

if TYPE_CHECKING:
    from tensorfs import raw

    type Stats = raw.QuantizationStats

type F32 = npt.NDArray[np.float32]
type Parts = dict[str, tuple[str, tuple[int, ...], bytes]]


class Overrides(TypedDict, total=False):
    """What one `encode` call changes of raw.quantize's keywords."""

    shape: Sequence[int]
    strategy: str
    params: Mapping[str, object]
    tensor_amax: float


class Refusal(Overrides, total=False):
    dtype: str


class Options(TypedDict):
    dtype: str
    shape: Sequence[int]
    strategy: str
    params: Mapping[str, object]
    tensor_amax: NotRequired[float]


class CaseRecord(TypedDict):
    name: str
    shape: tuple[int, ...]
    weight_sha256: str
    scale_sha256: str
