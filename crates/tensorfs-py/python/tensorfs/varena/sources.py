"""Immutable CPU snapshots for native Region Sources.

Snapshots contain the complete typed storage, including gaps between strided views.
Creating a Region copies these bytes into the native host Source. CUDA staging,
producer completion and retirement remain in the native core.
"""

from __future__ import annotations

import ctypes
import operator
from collections.abc import Sequence
from dataclasses import dataclass
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    import torch

# DLPack scalar descriptors: code, bits, lanes. Sub-byte and quantized storage
# need different view/conversion semantics and are deliberately not admitted here.
_DTYPES = {
    "bool": (6, 8, 1),
    "uint8": (1, 8, 1),
    "uint16": (1, 16, 1),
    "uint32": (1, 32, 1),
    "uint64": (1, 64, 1),
    "int8": (0, 8, 1),
    "int16": (0, 16, 1),
    "int32": (0, 32, 1),
    "int64": (0, 64, 1),
    "float16": (2, 16, 1),
    "bfloat16": (4, 16, 1),
    "float32": (2, 32, 1),
    "float64": (2, 64, 1),
    "complex64": (5, 64, 1),
    "complex128": (5, 128, 1),
}


def normalize_dtype(dtype: object) -> str:
    """Accept a canonical dtype name or its corresponding ``torch.dtype``."""
    name = str(dtype).removeprefix("torch.")
    if name not in _DTYPES:
        raise ValueError(f"unsupported Source dtype: {dtype}")
    return name


def dtype_descriptor(dtype: object) -> tuple[int, int, int]:
    """Return the native DLPack scalar descriptor for an admitted dtype."""
    return _DTYPES[normalize_dtype(dtype)]


def _dimensions(values: Sequence[int], name: str) -> tuple[int, ...]:
    result = tuple(operator.index(value) for value in values)
    if any(value < 0 or value >= 2**63 for value in result):
        raise ValueError(f"{name} must contain nonnegative int64 values")
    return result


def _contiguous_strides(shape: tuple[int, ...]) -> tuple[int, ...]:
    stride = 1
    strides = []
    for dim in reversed(shape):
        strides.append(stride)
        stride *= max(dim, 1)
    return tuple(reversed(strides))


@dataclass(frozen=True, slots=True, init=False)
class Source:
    """A read-only typed host snapshot, independent of its input tensor/buffer.

    ``strides`` are element strides; ``byte_offset`` addresses the first element
    in the complete snapshot. The caller must prevent concurrent input writes
    while a snapshot is copied. No tensor or mutable buffer remains an owner.
    """

    data: bytes
    dtype: str
    shape: tuple[int, ...]
    strides: tuple[int, ...]
    byte_offset: int

    def __new__(cls):
        raise TypeError("construct Source with from_buffer or from_tensor")

    @classmethod
    def from_buffer(
        cls,
        data: bytes | bytearray | memoryview,
        *,
        dtype: str,
        shape: Sequence[int],
        strides: Sequence[int] | None = None,
        byte_offset: int = 0,
    ) -> Source:
        """Copy native-endian host bytes and validate their complete typed view."""
        if not isinstance(data, (bytes, bytearray, memoryview)):
            raise TypeError("Source bytes must be bytes, bytearray or memoryview")
        if isinstance(data, memoryview) and not data.c_contiguous:
            raise ValueError("Source buffer must be C-contiguous")
        name = normalize_dtype(dtype)
        itemsize = dtype_descriptor(name)[1] // 8
        shape = _dimensions(shape, "shape")
        strides = (
            _contiguous_strides(shape)
            if strides is None
            else _dimensions(strides, "strides")
        )
        if len(shape) != len(strides):
            raise ValueError("shape and strides must have equal rank")
        # Contiguous strides can themselves overflow even when each dimension fits.
        if any(stride >= 2**63 for stride in strides):
            raise ValueError("strides must contain nonnegative int64 values")
        offset = operator.index(byte_offset)
        snapshot = bytes(data)
        if len(snapshot) % itemsize:
            raise ValueError("Source storage length must be divisible by dtype size")
        if offset < 0 or offset >= 2**64 or offset % itemsize:
            raise ValueError("byte_offset must be an aligned nonnegative uint64")
        extent = (
            0
            if 0 in shape
            else itemsize
            * (1 + sum((dim - 1) * stride for dim, stride in zip(shape, strides)))
        )
        if offset + extent > len(snapshot):
            raise ValueError("Source tensor view exceeds its host storage")
        source = object.__new__(cls)
        for key, value in (
            ("data", snapshot),
            ("dtype", name),
            ("shape", shape),
            ("strides", strides),
            ("byte_offset", offset),
        ):
            object.__setattr__(source, key, value)
        return source

    @classmethod
    def from_tensor(cls, tensor: torch.Tensor) -> Source:
        """Snapshot an ordinary dense CPU Tensor/Parameter and its storage view.

        Lazy conjugate/negative views require a value transformation, so callers
        must explicitly resolve them before requesting a storage snapshot.
        """
        import torch

        if type(tensor) not in (torch.Tensor, torch.nn.Parameter):
            raise TypeError("Source requires an ordinary Torch Tensor or Parameter")
        if tensor.device.type != "cpu" or tensor.layout != torch.strided:
            raise ValueError("Source tensor must have dense strided CPU storage")
        if tensor.is_nested or tensor.is_conj() or tensor.is_neg():
            raise ValueError("resolve nested/conjugate/negative tensor views first")
        name = normalize_dtype(tensor.dtype)
        storage = tensor.untyped_storage()
        count = storage.nbytes()
        # The storage object retains its allocation for the synchronous host copy.
        data = ctypes.string_at(storage.data_ptr(), count) if count else b""
        return cls.from_buffer(
            data,
            dtype=name,
            shape=tensor.shape,
            strides=tensor.stride(),
            byte_offset=tensor.storage_offset() * tensor.element_size(),
        )

    @property
    def nbytes(self) -> int:
        return len(self.data)

    def derive(self, dtype: str | torch.dtype) -> Source:
        """Convert the entire typed CPU storage into a new immutable version.

        Element strides and offsets survive dtype conversion. Existing Regions
        and escaped aliases retain their original snapshot and native Pin.
        """
        import torch

        target = normalize_dtype(dtype)
        if target == self.dtype:
            return self
        old_size = dtype_descriptor(self.dtype)[1] // 8
        new_size = dtype_descriptor(target)[1] // 8
        if self.data:
            # A private writable buffer avoids presenting immutable bytes as a
            # mutable Torch tensor. Only the resulting immutable copy escapes.
            values = torch.frombuffer(
                bytearray(self.data), dtype=getattr(torch, self.dtype)
            )
            converted = values.to(dtype=getattr(torch, target))
            storage = converted.untyped_storage()
            data = ctypes.string_at(storage.data_ptr(), storage.nbytes())
        else:
            data = b""
        return type(self).from_buffer(
            data,
            dtype=target,
            shape=self.shape,
            strides=self.strides,
            byte_offset=self.byte_offset // old_size * new_size,
        )
