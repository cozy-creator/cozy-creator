"""Names for this package's annotations, imported by type checkers only."""

from collections.abc import Callable
from types import ModuleType
from typing import TYPE_CHECKING, NotRequired, Protocol, TypedDict

import numpy as np
import numpy.typing as npt

if TYPE_CHECKING:
    import tensorfs
    from tensorfs import raw
    from tensorfs.api import QuantizeFit, QuantizeTensor
    from torch import Tensor

    type Parts = dict[str, tensorfs.Array]
    type Selection = QuantizeTensor
    type Fit = QuantizeFit
    type ArrayTensor = tensorfs.Array
    type EncodedTensor = tensorfs.Encoded
    type Checkpoint = tensorfs.Checkpoint
    type ComponentWriter = tensorfs.ComponentWriter
    type Control = tensorfs.ConversionControl

    class SvdqLinear(Protocol):
        """The frozen serving module's two names `historical_svdq` keeps."""

        @property
        def fake(self) -> Callable[[ModuleType, Tensor, Tensor], Tensor]: ...
        @property
        def LEVELS_MAX(self) -> float: ...

    class TensorFS(Protocol):
        """The parts of the `tensorfs` module the helpers are handed."""

        @property
        def Writer(self) -> type[tensorfs.Writer]: ...
        @property
        def ConversionControl(self) -> type[tensorfs.ConversionControl]: ...
        @property
        def Error(self) -> type[tensorfs.Error]: ...

    class Calibrated(Protocol):
        """One linear's calibration: per-channel `|x|` maxima and sampled input rows."""

        @property
        def xmax(self) -> Tensor: ...
        @property
        def rows(self) -> Tensor: ...

    class FitDetails(TypedDict):
        strategy: str
        alpha: float
        residual: raw.QuantizationStats
        calibration_error: float
        plain_nvfp4_error: float
        calibration_bytes_read: int
        required_buffer_bytes: int
        workers: int
        fit_seconds: float

    class Converted(TypedDict):
        """One tensor of a conversion's result."""

        component: str
        key: str
        source_bytes_read: int
        new_bytes_written: int
        relative_frobenius: float
        measured: raw.QuantizationStats
        fit: NotRequired[FitDetails]

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
