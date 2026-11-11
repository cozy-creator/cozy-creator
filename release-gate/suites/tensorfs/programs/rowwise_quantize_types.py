"""Names for this package's annotations, imported by type checkers only."""

from typing import TYPE_CHECKING, Protocol, TypedDict

import numpy as np
import numpy.typing as npt

if TYPE_CHECKING:
    import tensorfs
    from tensorfs import _ext
    from tensorfs.api import QuantizationResult, QuantizedTensor, QuantizeTensor

    type MetadataInput = _ext.MetadataInput
    type Converted = QuantizedTensor
    type Conversion = QuantizationResult

    type Selection = QuantizeTensor

    class TensorFS(Protocol):
        """The parts of the `tensorfs` module `cancel_rowwise` is handed."""

        @property
        def Writer(self) -> type[tensorfs.Writer]: ...
        @property
        def ConversionControl(self) -> type[tensorfs.ConversionControl]: ...
        @property
        def NetworkError(self) -> type[tensorfs.NetworkError]: ...

type F32 = npt.NDArray[np.float32]


class BadSource(TypedDict, total=False):
    """How one refusal case spoils the rowwise source."""

    params: "dict[str, MetadataInput]"
    scale_shape: tuple[int, ...]
    weight_dtype: str
    scale_dtype: str
    extra: bool
    nonfinite: bool


class Summary(TypedDict):
    target: str
    chunk_rows: int
    read_bytes: int
    new_bytes: int
    identity: str
    digests: dict[str, str]


class Refusal(TypedDict):
    case: str
    code: str
    read_bytes: int

