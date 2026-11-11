"""Names for this package's annotations, imported by type checkers only."""

from collections.abc import Callable
from types import ModuleType
from typing import TYPE_CHECKING, NotRequired, Protocol, TypedDict

import numpy as np
import numpy.typing as npt

if TYPE_CHECKING:
    import tensorfs
    from tensorfs.api import FitResult, QuantizedTensor
    from torch import Tensor

    type Converted = QuantizedTensor
    type FitDetails = FitResult

    type Parts = dict[str, tensorfs.Array]
    type ArrayTensor = tensorfs.Array
    type EncodedTensor = tensorfs.Encoded

    class SvdqLinear(Protocol):
        """The frozen serving module's two names `historical_svdq` keeps."""

        @property
        def fake(self) -> Callable[[ModuleType, Tensor, Tensor], Tensor]: ...
        @property
        def LEVELS_MAX(self) -> float: ...

    class Calibrated(Protocol):
        """One linear's calibration: per-channel `|x|` maxima and sampled input rows."""

        @property
        def xmax(self) -> Tensor: ...
        @property
        def rows(self) -> Tensor: ...

    class Facts(TypedDict):
        """`check_selected`'s account of one selected weight."""

        fit: FitDetails
        relative_frobenius: float
        logical_energy: float
        residual_energy: float
        residual_packing_sha256: str
        stored_parts: dict[str, str]
        lowrank_error: NotRequired[float]
        exact_rank32_error: NotRequired[float]


type F32 = npt.NDArray[np.float32]
#: A residual decoded at F32 or F64.
type Floats = npt.NDArray[np.float32 | np.float64]
