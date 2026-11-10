"""Names for this package's annotations, imported by type checkers only."""

from collections.abc import Mapping, Sequence
from typing import TYPE_CHECKING, NotRequired, TypedDict

import numpy as np
import numpy.typing as npt

if TYPE_CHECKING:
    import tensorfs
    from tensorfs import raw
    from tensorfs.api import QuantizeTensor

    type Selection = QuantizeTensor
    type EncodedTensor = tensorfs.Encoded
    type Stats = raw.QuantizationStats

    class Converted(TypedDict):
        """One tensor of a conversion's result."""

        component: str
        key: str
        source_bytes_read: int
        new_bytes_written: int
        relative_frobenius: float
        measured: raw.QuantizationStats

    class Conversion(TypedDict):
        """`Writer.quantize`'s result."""

        identity: str
        encoded_keys: int
        reused_keys: int
        source_bytes_read: int
        calibration_bytes_read: int
        required_buffer_bytes: int
        new_bytes_written: int
        saturated_elements: int
        worst_relative_frobenius: float | None
        tensors: list[Converted]

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


class Budget(TypedDict):
    max_buffer_bytes: int
    max_new_bytes: int


class Record(TypedDict):
    scheme: str
    name: str
    shape: tuple[int, ...]
    parts: dict[str, str]
