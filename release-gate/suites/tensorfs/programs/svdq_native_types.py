"""Names for `svdq_native.py`'s annotations, imported by type checkers only."""

from typing import TYPE_CHECKING, NotRequired, TypedDict

import numpy as np
import numpy.typing as npt

if TYPE_CHECKING:
    import tensorfs
    from tensorfs import _ext
    from tensorfs.api import (
        FitResult,
        QuantizationResult,
        QuantizedTensor,
        QuantizeFit,
        QuantizeTensor,
    )

    type MetadataInput = _ext.MetadataInput
    type Converted = QuantizedTensor
    type Conversion = QuantizationResult
    type FitDetails = FitResult

    type Parts = dict[str, tensorfs.Array]
    type Selection = QuantizeTensor
    type Fit = QuantizeFit
    type ArrayTensor = tensorfs.Array
    type EncodedTensor = tensorfs.Encoded
    type Checkpoint = tensorfs.Checkpoint
    type ComponentWriter = tensorfs.ComponentWriter
    type Control = tensorfs.ConversionControl

    class Facts(TypedDict):
        """`check_selected`'s account of one selected weight."""

        fit: FitDetails
        relative_frobenius: float
        logical_energy: float
        residual_energy: float
        residual_packing_sha256: str
        stored_parts: dict[str, str]
        stored_pre_calibration_error: NotRequired[float]
        multiply_vs_divide_bf16_differences: NotRequired[int]
        lowrank_error: NotRequired[float]
        exact_rank32_error: NotRequired[float]

    class Run(TypedDict):
        workspace: int
        fit_seconds: float
        result: Conversion
        facts: dict[str, Facts]
        checkpoint: str


type F32 = npt.NDArray[np.float32]
#: A residual decoded at F32 or F64.
type Floats = npt.NDArray[np.float32 | np.float64]


class Served(TypedDict):
    stored_pre_calibration_error: float
    multiply_vs_divide_bf16_differences: int


class Refusing(TypedDict, total=False):
    """What one `refused` call changes of `Writer.quantize`'s keywords."""

    control: "Control"
    component: str
    max_buffer_bytes: int
    max_new_bytes: int
    max_relative_frobenius: float


class Budget(TypedDict, total=False):
    component: str
    max_buffer_bytes: int
    max_new_bytes: int
    max_relative_frobenius: float


class Observation(TypedDict):
    key: str
    shape: tuple[int, ...]
    strides: tuple[int, ...]
    c_contiguous: bool
    bytes: int
