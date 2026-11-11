"""Names for this package's annotations, imported by type checkers only."""

from typing import TYPE_CHECKING, TypedDict

import numpy as np
import numpy.typing as npt

if TYPE_CHECKING:
    import tensorfs
    from tensorfs.api import QuantizationResult, QuantizedTensor, QuantizeTensor

    type Converted = QuantizedTensor
    type Conversion = QuantizationResult

    type Selection = QuantizeTensor
    type EncodedTensor = tensorfs.Encoded
    type ArrayTensor = tensorfs.Array

type F32 = npt.NDArray[np.float32]
type Parts = dict[str, tuple[str, tuple[int, ...], bytes]]


class Budget(TypedDict):
    max_buffer_bytes: int
    max_new_bytes: int
