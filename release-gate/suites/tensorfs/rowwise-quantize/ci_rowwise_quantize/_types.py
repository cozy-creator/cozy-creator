"""Names for this package's annotations, imported by type checkers only."""

from typing import TYPE_CHECKING, Protocol, TypedDict

import numpy as np
import numpy.typing as npt

if TYPE_CHECKING:
    import tensorfs
    from tensorfs import raw
    from tensorfs.api import QuantizeTensor

    type Selection = QuantizeTensor

    class Ext(Protocol):
        __file__: str

    class TensorFS(Protocol):
        """The parts of the `tensorfs` module `cancel_rowwise` is handed."""

        @property
        def Writer(self) -> type[tensorfs.Writer]: ...
        @property
        def ConversionControl(self) -> type[tensorfs.ConversionControl]: ...
        @property
        def NetworkError(self) -> type[tensorfs.NetworkError]: ...
        @property
        def _ext(self) -> Ext: ...

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


class BadSource(TypedDict, total=False):
    """How one refusal case spoils the rowwise source."""

    params: dict[str, object]
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

