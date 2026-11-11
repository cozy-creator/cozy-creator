import io
from collections.abc import Callable, Mapping, Sequence
from os import PathLike
from types import TracebackType
from typing import Literal, Protocol, TypedDict
from typing_extensions import Buffer, NotRequired

from typing_extensions import CapsuleType

from . import _ext
from .raw import QuantizationStats
from ._ext import Checkpoint, ClassRef, Component, ConversionControl, TokenProvider, _CheckpointFile, ConfigInput, MetadataInput, _Resolved

StrPath = str | PathLike[str]
Token = str | TokenProvider | Callable[[str, str, Mapping[str, str], str | None], Mapping[str, str]]

class DLPack(Protocol):
    def __dlpack__(self) -> CapsuleType: ...

def configure(*, endpoint: str | None = None, cache_dir: StrPath | None = None, token: Token | None = None,
              ca_der: bytes | None = None) -> None: ...
def open(name_or_path: StrPath, *, release: str | None = None, lane: str | None = None,
         gpus: Sequence[str] | None = None,
         endpoint: str | None = None, local_files_only: bool = False, cache_dir: StrPath | None = None,
         token: Token | None = None, lazy: bool = True) -> Checkpoint: ...
def view(header: bytes, own: Sequence[bytes] = (), *, endpoint: str | None = None,
         local_files_only: bool = False, cache_dir: StrPath | None = None,
         token: Token | None = None) -> Checkpoint: ...
def materialize(source: str | Checkpoint, *, local_dir: StrPath | None = None,
                format: str = "cozytensors", decode: bool = False,
                release: str | None = None, lane: str | None = None,
                components: Sequence[str] | None = None, cache_dir: StrPath | None = None,
                local_files_only: bool = False, token: Token | None = None,
                endpoint: str | None = None) -> str: ...
def download_file(model_id: str | None = None, filename: str | None = None, *, repo_id: str | None = None, release: str | None = None, lane: str | None = None,
                 cache_dir: StrPath | None = None, local_dir: StrPath | None = None,
                 local_files_only: bool = False, token: Token | None = None,
                 endpoint: str | None = None) -> str: ...
def upload_folder(*, model_id: str | None = None, repo_id: str | None = None, folder_path: StrPath, lane: str,
                  token: Token | None = None, endpoint: str | None = None,
                  cache_dir: StrPath | None = None) -> str:
    """Uploads the folder (one `.cozytensors` file and its ancillary files) as a new checkpoint of
    `model_id` in lane `lane`, private until a release names it. The checkpoint's id,
    `sha256:<tree hex>`."""
def convert_to_cozytensors(source: str | Sequence[str], *, converter: str | None = None, source_variant: str | None = None,
            cache_dir: StrPath | None = None, token: Token | None = None,
            progress: Callable[[str, int, int], object] | None = None) -> Checkpoint: ...

class CheckpointFile(io.RawIOBase):
    """The checkpoint as one read-only, seekable file: its header, then its chunks and assets in
    header order at 4 KiB-aligned offsets, assembled from the store on demand; nothing is written."""
    def __init__(self, native: _CheckpointFile) -> None: ...
    def readable(self) -> bool: ...
    def seekable(self) -> bool: ...
    def readinto(self, buffer: Buffer) -> int: ...
    def seek(self, offset: int, whence: int = 0) -> int: ...
    def tell(self) -> int: ...
    def close(self) -> None: ...

class Endpoint:
    def __init__(self, url: str, token: Token | None = None, *, ca_der: bytes | None = None, cache_dir: StrPath | None = None) -> None: ...
    def resolve(self, model_id: str | None = None, *, repo_id: str | None = None, release: str | None = None, lane: str | None = None,
                gpus: Sequence[str] | None = None, plain: bool = False) -> _Resolved:
        """The checkpoint of `release` (none: the newest) in `lane` (none: what `gpus` run best; none:
        the CPU): `{release, lane, checkpoint, header, reasons}`."""

class ComponentWriter:
    def __init__(self, native: _ext.ComponentWriter) -> None: ...
    def put(self, key: str, data: Buffer | DLPack, *, dtype: str | None = None, shape: Sequence[int] | None = None) -> None: ...
    def put_encoded(self, key: str, *, dtype: str, shape: Sequence[int], scheme: str,
                    params: Mapping[str, MetadataInput], parts: Mapping[str, tuple[str, Sequence[int], Buffer]]) -> None: ...
    def copy(self, key: str, source: Component, *, as_key: str | None = None) -> None: ...
    def asset(self, name: str, data: Buffer) -> None: ...
    def order(self, keys: Sequence[str]) -> None: ...
    def has(self, key: str) -> bool: ...

class NormalizeBf16Tensor(TypedDict):
    component: str
    key: str
    source_dtype: str
    shape: Sequence[int]

class QuantizeFit(TypedDict):
    seed: int
    component: str
    xmax: str
    rows: str

class QuantizeEncoding(TypedDict):
    scheme: str
    params: NotRequired[Mapping[str, MetadataInput]]

class QuantizeTensor(TypedDict):
    component: str
    key: str
    source_dtype: str
    shape: Sequence[int]
    dtype: str
    encoding: QuantizeEncoding
    strategy: str
    fit: NotRequired[QuantizeFit]

class FitResult(TypedDict):
    strategy: str
    alpha: float
    residual: QuantizationStats
    calibration_error: float
    plain_nvfp4_error: float
    calibration_bytes_read: int
    required_buffer_bytes: int
    workers: int
    fit_seconds: float

class QuantizedTensor(TypedDict):
    component: str
    key: str
    source_bytes_read: int
    new_bytes_written: int
    relative_frobenius: float
    measured: QuantizationStats
    fit: NotRequired[FitResult]

class NormalizationResult(TypedDict):
    """`Writer.normalize_bf16`'s result (crates/tensorfs `NormalizationResult`)."""
    identity: str
    converted_keys: int
    reused_keys: int
    source_bytes_read: int
    logical_source_bytes_read: int
    new_bytes_written: int
    required_buffer_bytes: int
    worst_relative_frobenius: float | None
    tensors: list[QuantizedTensor]

class QuantizationResult(TypedDict):
    """`Writer.quantize`'s result (crates/tensorfs `QuantizationResult`)."""
    identity: str
    encoded_keys: int
    reused_keys: int
    source_bytes_read: int
    calibration_bytes_read: int
    required_buffer_bytes: int
    new_bytes_written: int
    saturated_elements: int
    worst_relative_frobenius: float | None
    tensors: list[QuantizedTensor]

class Writer:
    metadata: dict[str, MetadataInput]
    local_checkpoint: Checkpoint | None
    def __init__(self, dest: StrPath | None = None, *, base: Checkpoint | None = None,
                 resume: str | None = None, model_id: str | None = None, lane: str | None = None, repo_id: str | None = None,
                 endpoint: str | None = None, token: Token | None = None,
                 cache_dir: StrPath | None = None) -> None:
        """`dest` is a complete portable checkpoint file. With `model_id` and `lane`,
        `commit()` saves a new checkpoint under that model. The legacy repo_id keyword is a
        finite compatibility bridge with planned removal at the 0.25.0 consumer cut."""
    def __enter__(self) -> Writer: ...
    def __exit__(self, exc_type: type[BaseException] | None, exc: BaseException | None,
                 traceback: TracebackType | None) -> None: ...
    def pipeline(self, cls: ClassRef | tuple[str, str, str] | Literal[""] | None = None, config: Mapping[str, ConfigInput] | None = None) -> None: ...
    def component(self, name: str, *, cls: ClassRef | tuple[str, str, str] | Literal[""] | None = None, config: Mapping[str, ConfigInput] | None = None,
                  config_file: str | None = None) -> ComponentWriter: ...
    def commit(self) -> Checkpoint: ...
    def abandon(self) -> None: ...
    def has(self, component: str, key: str) -> bool: ...
    def normalize_bf16(self, source: Checkpoint, tensors: Sequence[NormalizeBf16Tensor], *,
                      max_buffer_bytes: int, max_new_bytes: int,
                      control: ConversionControl) -> NormalizationResult: ...
    def quantize(self, source: Checkpoint, tensors: Sequence[QuantizeTensor], *,
                 max_buffer_bytes: int, max_new_bytes: int, control: ConversionControl,
                 max_relative_frobenius: float | None = None,
                 calibration: Checkpoint | None = None,
                 component: str | None = None) -> QuantizationResult: ...
