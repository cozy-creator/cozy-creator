"""Names for this package's annotations, imported by type checkers only."""

from collections.abc import Mapping, Sequence
from typing import TYPE_CHECKING, Protocol, TypedDict

import numpy as np
import numpy.typing as npt

if TYPE_CHECKING:
    import tensorfs
    from tensorfs.api import (
        NormalizationResult,
        NormalizeBf16Tensor,
        QuantizationResult,
        QuantizedTensor,
        QuantizeTensor,
    )

    type Converted = QuantizedTensor
    type Conversion = QuantizationResult
    type Normalization = NormalizationResult

    type Selected = NormalizeBf16Tensor
    type Quantize = QuantizeTensor
    type Writer = tensorfs.Writer
    type Checkpoint = tensorfs.Checkpoint
    type Control = tensorfs.ConversionControl
    type ArrayTensor = tensorfs.Array
    type EncodedTensor = tensorfs.Encoded

    class Raw(Protocol):
        def quantize(
            self, scheme: str, values: bytes, *, dtype: str, shape: Sequence[int], strategy: str,
            params: Mapping[str, object] | None = None, tensor_amax: float | None = None,
        ) -> tensorfs.Encoded: ...

    class TensorFS(Protocol):
        """The parts of the `tensorfs` module the helpers are handed."""

        @property
        def Writer(self) -> type[tensorfs.Writer]: ...
        @property
        def ConversionControl(self) -> type[tensorfs.ConversionControl]: ...
        @property
        def InvalidError(self) -> type[tensorfs.InvalidError]: ...
        @property
        def DiskFullError(self) -> type[tensorfs.DiskFullError]: ...
        @property
        def NotFoundError(self) -> type[tensorfs.NotFoundError]: ...
        @property
        def NetworkError(self) -> type[tensorfs.NetworkError]: ...
        @property
        def DecodeError(self) -> type[tensorfs.DecodeError]: ...
        @property
        def UnsupportedError(self) -> type[tensorfs.UnsupportedError]: ...
        @property
        def ConflictError(self) -> type[tensorfs.ConflictError]: ...
        @property
        def raw(self) -> Raw: ...

type JSON = str | int | float | bool | None | list[JSON] | dict[str, JSON]
type F32 = npt.NDArray[np.float32]
#: A source case: what it decodes to, the stored bytes a conversion reads, and its logical bytes.
type Case = tuple[F32, int, int]


class Oracle(Protocol):
    """The frozen `oracle` module the helpers are handed."""

    def to_f32(self, raw: bytes | bytearray | memoryview, dtype: str, shape: list[int]) -> F32: ...
    def from_f32(self, values: F32, dtype: str) -> bytes: ...
    def normalize(self, values: F32) -> tuple[bytes, dict[str, float | int]]: ...
    def element(self, code: int, dtype: str) -> float: ...
    def rowwise(
        self, codes: bytes, weight_dtype: str, scales: bytes, scale_dtype: str, shape: tuple[int, int]
    ) -> F32: ...


class Limits(TypedDict, total=False):
    buffer: int
    allowance: int


class Timing(TypedDict):
    wall_seconds: float
    process_cpu_seconds: float
    process_maxrss_before_bytes: int
    process_maxrss_after_bytes: int


class Pass(TypedDict):
    result: Normalization
    timing: Timing
    preflight_refusals: list[str]


class Refused(TypedDict):
    case: str
    code: str
    detail: str


class Mixed(TypedDict):
    initial_normalization: Normalization
    initial_quantization: Conversion
    resumed_normalization: Normalization
    resumed_quantization: Conversion


class Reservation(TypedDict):
    refused_operation: str
    detail: str
    progress: tuple[int, int, int]
    unwritten_key_absent: bool


class Reversed(Reservation):
    cancel_progress: tuple[int, int, int]
    cancel_join_seconds: float


class Stamp(TypedDict, total=False):
    """What `run` adds to either mode's report."""

    mode: str
    tensorfs: str
    numpy: str
    native_sha256: str
    oracle_sha256: str
    strategy: str
    resource_scope: str
    traffic_scope: str


class Basic(Stamp):
    cases: int
    reports: list[Pass]
    numerical_refusals: list[Refused]
    mixed_resume: Mixed
    part_sha256: dict[str, str]


class Cancel(Stamp):
    normalization_reservation: Reservation
    quantization_reservation: Reversed
    fixture_source_bytes: int
    fixture_generated_window_bytes: int
    cancel_progress: tuple[int, int, int]
    cancel_join_seconds: float
    elapsed_to_join_seconds: float
    resumed: Normalization
    resume_timing: Timing
    output_sha256: str
