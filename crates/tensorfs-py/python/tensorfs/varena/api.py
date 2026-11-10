"""Public ABI 1: Gpu, Region, Allocation, Cache and Plan.

All calls resolve through the native core already linked by tensorfs._ext. CDLL
releases the GIL on calls, including callback drains. Host Region bytes are copied.
Raw addresses require an owning Use/Allocation/Cache and correct CUDA stream order.
Use context managers or explicit close; a finalizer cannot prove GPU completion.
"""

from __future__ import annotations

import ctypes as C
import operator
from collections.abc import Callable, Iterator, Mapping, Sequence
from contextlib import contextmanager, suppress
from types import TracebackType
from typing import TYPE_CHECKING, Literal, TypedDict

from .. import _ext
from .sources import Source, dtype_descriptor

if TYPE_CHECKING:
    import torch
    from _ctypes import _CDataType
    from ctypes import _CArgObject, _NamedFuncPointer

    from typing_extensions import Self

    #: What a declared native call takes: a ctypes value, a `byref`, or a Python int/bytes it converts.
    _Arg = _CDataType | _CArgObject | int | bytes | None


class SplitSelection(TypedDict):
    """`Gpu.split_select`'s choice: pass it to `Plan.split`."""

    index: int
    bytes: int
    measured: int
    budget: int
    generation: int
    id: str


class Error(RuntimeError):
    def __init__(self, message: str, requested_bytes: int = 0) -> None:
        super().__init__(message)
        self.requested_bytes = requested_bytes


class Exhausted(Error):
    # Native recovery already exhausted its admitted choices. A consumer may not
    # replay partially executed application code to turn this into another attempt.
    sticky = False


class Capturing(Error):
    pass


class SourceError(Error):
    pass


class Fault(Error):
    sticky = True


class Invalid(Error, ValueError):
    pass


class Unsupported(Error):
    pass


class Busy(Error):
    pass


_errors = {
    1: Exhausted,
    2: Capturing,
    3: SourceError,
    4: Fault,
    5: Invalid,
    6: Unsupported,
    7: Busy,
}
_u = C.c_uint64
_h = C.POINTER(_u)
_drop_type = C.CFUNCTYPE(None, C.c_void_p)
_shrink_type = C.CFUNCTYPE(C.c_int32, C.c_void_p, _u, _u)


class _Shrinker(C.Structure):
    _fields_ = [("shrink", _shrink_type), ("drop", _drop_type), ("context", C.c_void_p)]


class _CacheInfo(C.Structure):
    _fields_ = [
        (name, _u)
        for name in (
            "base",
            "granularity",
            "reserved",
            "mapped",
            "live",
            "pending",
            "ready",
        )
    ]


class _RetirementStats(C.Structure):
    _fields_ = [
        (name, _u)
        for name in ("queued", "retired", "quarantined", "abandoned_cache_bytes")
    ]


class _Stats(C.Structure):
    _fields_ = [
        (name, _u)
        for name in (
            "weights_resident",
            "working_live",
            "working_capacity",
            "working_mapped",
            "working_pending",
            "working_free",
            "working_peak",
            "pools",
            "caches",
            "cache_live",
            "cache_pending",
            "cache_ready",
            "allocations",
            "frees",
            "driver_calls",
            "driver_ooms",
        )
    ]


class _Room(C.Structure):
    _fields_ = [(name, _u) for name in ("capacity", "working_live", "pinned", "weights_resident", "free")]


class _SplitChoice(C.Structure):
    _fields_ = [("id", C.c_char_p), ("bytes", _u)]


class _SplitSelection(C.Structure):
    _fields_ = [(name, _u) for name in ("index", "bytes", "measured", "budget", "generation")]


class _SplitEnd(C.Structure):
    _fields_ = [(name, _u) for name in ("peak", "counted")]


class _Phase(C.Structure):
    _fields_ = [("name", C.c_char_p), ("regions", _h), ("count", C.c_size_t)]


class _HostPart(C.Structure):
    _fields_ = [("data", C.c_void_p), ("bytes", C.c_size_t)]


# Symbol lookup traverses _ext's DT_NEEDED dependencies. Never dlopen a second
# allocator by an unrelated absolute path, or statically embed a core in this module.
_lib = C.CDLL(_ext.__file__)


def _declare(name: str, args: list[type[_CDataType]], result: type[_CDataType] = C.c_int32) -> _NamedFuncPointer:
    function: _NamedFuncPointer = getattr(_lib, "vr_v1_" + name)
    function.argtypes = args
    function.restype = result
    return function


_declare("abi_version", [], C.c_uint32)
_declare("core_identity", [], _u)
_declare("last_error", [], C.c_char_p)
_declare("error_requested_bytes", [], _u)
_declare("retirement_stats", [C.POINTER(_RetirementStats), C.c_size_t])
_abi: dict[str, list[type[_CDataType]]] = {
    "defer_close": [_u],
    "gpu_open": [C.c_int32, _h],
    "gpu_close": [_u],
    "region_from_host": [_u, C.c_void_p, C.c_size_t, _h],
    "region_borrow_host_parts": [_u, C.POINTER(_HostPart), C.c_size_t, _h, C.c_void_p, C.c_void_p, _h],
    "region_ptr": [_u, _h],
    "region_close": [_u],
    "use": [_u, _h, C.c_size_t, _u, _h],
    "use_ptr": [_u, C.c_size_t, _h],
    "use_wait_ready": [_u],
    "done": [_u],
    "alloc": [_u, _u, _u, _h],
    "allocation_ptr": [_u, _h],
    "record_stream": [_u, _u],
    "free": [_u],
    "cache_new": [_u, _u, _Shrinker, _h],
    "cache_info": [_u, C.POINTER(_CacheInfo), C.c_size_t],
    "cache_commit": [_u, _u, _u],
    "cache_release": [_u, _u, _u, _u],
    "cache_close": [_u],
    "plan_new": [_u, C.POINTER(_Phase), C.c_size_t, _h],
    "plan_activate": [_u, C.c_char_p],
    "plan_deactivate": [_u],
    "phase_enter": [_u, C.c_char_p, C.c_char_p],
    "phase_leave": [_u, C.c_int32],
    "plan_close": [_u],
    "stats": [_u, C.POINTER(_Stats), C.c_size_t],
    "room": [_u, C.POINTER(_Room), C.c_size_t],
    "region_set_priority": [_u, C.c_int32],
    "split_select": [_u, C.c_char_p, C.POINTER(_SplitChoice), C.c_size_t, _u, C.POINTER(_SplitSelection), C.c_size_t],
    "split_begin": [_u, C.c_char_p, C.c_char_p, _u, _u, _h],
    "split_end": [_u, _u, C.c_int32, C.POINTER(_SplitEnd), C.c_size_t],
}
for _name, _args in _abi.items():
    _declare(_name, _args)

if (
    _lib.vr_v1_abi_version() != 1
    or _lib.vr_v1_core_identity() != _ext._varena_core_identity()
):
    raise ImportError(
        "TensorFS and public Varena must resolve the same ABI 1 allocator core"
    )


def core_identity() -> int:
    return int(_lib.vr_v1_core_identity())


def retirement_stats() -> dict[str, int]:
    """Deferred work includes in-flight jobs; live Cache abandonment is explicit."""
    value = _RetirementStats()
    _call("retirement_stats", C.byref(value), C.sizeof(value))
    return _fields(value)


def _call(name: str, *args: _Arg) -> None:
    code = getattr(_lib, "vr_v1_" + name)(*args)
    if code:
        message = _lib.vr_v1_last_error().decode("utf-8", "replace")
        raise _errors.get(code, Error)(message, _lib.vr_v1_error_requested_bytes())


def _uint(value: int) -> int:
    value = operator.index(value)
    if not 0 <= value < 2**64:
        raise ValueError("expected a uint64 value")
    return value


def _text(value: str) -> bytes:
    if "\0" in value:
        raise ValueError("strings cannot contain NUL")
    return value.encode("utf-8")


def _new(name: str, *args: _Arg) -> int:
    out = _u()
    _call(name, *args, C.byref(out))
    return out.value


def _fields(value: C.Structure) -> dict[str, int]:
    return {field[0]: getattr(value, field[0]) for field in value._fields_}


class _Owned:
    _close: str

    def __init__(self, handle: int) -> None:
        self._handle = handle

    def close(self) -> None:
        if self._handle:
            _call(self._close, self._handle)
            self._handle = 0

    def defer_close(self) -> None:
        if self._handle:
            _call("defer_close", self._handle)
            self._handle = 0

    def __enter__(self) -> Self:
        if not self._handle:
            raise Invalid("handle is closed")
        return self

    def __exit__(
        self, kind: type[BaseException] | None, error: BaseException | None, traceback: TracebackType | None
    ) -> None:
        self.close()

    def __del__(self) -> None:
        # Nonblocking ownership transfer, including temporary capture/event refusal.
        with suppress(Error, AttributeError):
            self.defer_close()


class Gpu(_Owned):
    _close = "gpu_close"

    def __init__(self, device: int = 0) -> None:
        device = operator.index(device)
        if not 0 <= device < 2**31:
            raise ValueError("device must be a nonnegative int32")
        super().__init__(_new("gpu_open", device))
        self.device = device

    def region(self, data: Source | bytes | bytearray | memoryview) -> Region:
        host = data.data if isinstance(data, Source) else bytes(data)
        region = Region(_new("region_from_host", self._handle, host, len(host)), self)
        region.nbytes = len(host)
        return region

    def region_parts(self, parts: Sequence[tuple[int, int]], keep: object) -> tuple[Region, tuple[int, ...]]:
        """One Region over host ranges, each (address, bytes), at 256-byte-aligned offsets, filled
        from them with no host copy. Native holds `keep` until its last fill and handle let go;
        the ranges must stay valid and unchanged while it does.
        """
        hosts = (_HostPart * len(parts))(*(_HostPart(address, size) for address, size in parts))
        offsets = (_u * len(parts))()
        drop, context = _ext._varena_public_keep(keep)
        region = Region(_new("region_borrow_host_parts", self._handle, hosts, len(hosts), offsets, drop, context), self)
        region.nbytes = max(offset + size for offset, (_, size) in zip(offsets, parts))
        return region, tuple(offsets)

    def use(self, regions: Sequence[Region], stream: int) -> Use:
        """Name the actual consumer stream; every other consumer must join it before close."""
        ids = (_u * len(regions))(*(region._handle for region in regions))
        return Use(
            _new("use", self._handle, ids, len(ids), _uint(stream)),
            self,
            list(regions),
            stream,
        )

    def alloc(self, bytes: int, stream: int) -> Allocation:
        return Allocation(
            _new("alloc", self._handle, _uint(bytes), _uint(stream)), self, stream
        )

    def cache(
        self, reserve: int, shrink: Callable[[CacheView, int], bool] | None = None
    ) -> Cache:
        reserve = _uint(reserve)
        handle = _new("cache_new", self._handle, reserve, _Shrinker())
        cache = Cache(handle, self)
        if shrink is not None:
            # Reuse the native weak PyO3 callback adapter. It uses try_attach during
            # interpreter shutdown; no raw CFUNCTYPE callback can outlive Python.
            cache._callback = lambda requested: shrink(
                CacheView(handle, self), requested
            )
            try:
                cache._owner = _ext._varena_public_cache_owner(
                    handle, self.device, cache._callback
                )
            except _ext.CacheOwnerDeferredError as error:
                exception = (
                    Capturing if getattr(error, "reason", "") == "capture" else Busy
                )
                raise exception(str(error)) from error
        return cache

    def plan(self, phases: Mapping[str, Sequence[Region]]) -> Plan:
        names = [_text(name) for name in phases]
        ids = [
            (_u * len(regions))(*(r._handle for r in regions))
            for regions in phases.values()
        ]
        descriptors = (_Phase * len(names))(
            *(_Phase(name, handles, len(handles)) for name, handles in zip(names, ids))
        )
        return Plan(_new("plan_new", self._handle, descriptors, len(descriptors)), self)

    def stats(self) -> dict[str, int]:
        value = _Stats()
        _call("stats", self._handle, C.byref(value), C.sizeof(value))
        return _fields(value)

    def room(self) -> dict[str, int]:
        """What this GPU has right now. A report: nothing is reserved."""
        value = _Room()
        _call("room", self._handle, C.byref(value), C.sizeof(value))
        return _fields(value)

    def split_select(self, key: str, choices: Sequence[tuple[str, int]], stream: int) -> SplitSelection:
        """Legal levels, largest first, as (id, estimated full bytes). The core picks the largest
        under its budget, reported with its `id`; pass the result to `Plan.split`. Nothing is reserved.
        """
        menu = (_SplitChoice * len(choices))(*(_SplitChoice(_text(id), _uint(bytes)) for id, bytes in choices))
        value = _SplitSelection()
        _call("split_select", self._handle, _text(key), menu, len(menu), _uint(stream), C.byref(value), C.sizeof(value))
        return SplitSelection(
            index=value.index, bytes=value.bytes, measured=value.measured, budget=value.budget,
            generation=value.generation, id=choices[value.index][0],
        )


def open(device: int = 0) -> Gpu:
    return Gpu(device)


class Region(_Owned):
    _close = "region_close"
    #: The bytes it covers, set by the Gpu call that made it.
    nbytes: int

    def __init__(self, handle: int, gpu: Gpu) -> None:
        super().__init__(handle)
        self.gpu = gpu

    def set_priority(self, priority: Literal["normal", "high"]) -> None:
        """An idle "high" Region goes only once owner Caches and peers were asked to give memory."""
        _call("region_set_priority", self._handle, ("normal", "high").index(priority))

    def tensor(
        self,
        *,
        dtype: str | torch.dtype,
        shape: Sequence[int],
        strides: Sequence[int] | None = None,
        byte_offset: int = 0,
        stream: int,
    ) -> torch.Tensor:
        """Export typed storage, ordered on an actual explicit CUDA stream.

        Install tensorfs.varena.torch first. The tensor storage owns its native
        Pin independently of this Region and the temporary Use. Additional
        consumer streams need dependencies and Tensor.record_stream as usual.
        """
        with self.gpu.use([self], stream) as used:
            return used.tensor(
                dtype=dtype,
                shape=shape,
                strides=strides,
                byte_offset=byte_offset,
                stream=stream,
            )


class Split:
    """One selected level, measured until the end of this `with`. `peak` is what it used and `counted`
    says leaving the phase completed publishes that. An exception keeps no measure.
    """

    peak = counted = 0

    def __init__(self, plan: Plan, handle: int) -> None:
        self.plan, self.handle = plan, handle

    def __enter__(self) -> Split:
        return self

    def __exit__(
        self, kind: type[BaseException] | None, error: BaseException | None, traceback: TracebackType | None
    ) -> None:
        value = _SplitEnd()
        _call("split_end", self.plan._handle, self.handle, int(kind is None), C.byref(value), C.sizeof(value))
        self.peak, self.counted = value.peak, value.counted


class Use(_Owned):
    """Retain weights and order their producers on the supplied consumer stream.

    Success can precede GPU completion. Other reader streams need their own Use or
    a CUDA dependency behind this stream before reading, then must join it before done.
    """

    _close = "done"

    def wait_ready(self) -> None:
        """Await this Use's producer completion and checked resource retirement.

        Call outside capture/replay. CDLL releases the GIL; native custody is
        retained across concurrent close. This neither waits for consumers nor
        promises capture admission while unrelated producers are active.
        """
        _call("use_wait_ready", self._handle)

    def __init__(
        self, handle: int, gpu: Gpu, regions: list[Region], stream: int
    ) -> None:
        super().__init__(handle)
        self.gpu, self.regions, self.stream = gpu, regions, stream

    def ptr(self, index: int = 0) -> int:
        if not self._handle:
            raise Invalid("Use has ended")
        return _new("use_ptr", self._handle, _uint(index))

    def tensor(
        self,
        index: int = 0,
        *,
        dtype: str | torch.dtype,
        shape: Sequence[int],
        strides: Sequence[int] | None = None,
        byte_offset: int = 0,
        stream: int | None = None,
    ) -> torch.Tensor:
        """Export an independently pinned typed Torch tensor on this Use's stream.

        An explicit stream override orders the same Source producer there before
        export. Record other streams on the tensor before final alias release,
        after establishing CUDA dependencies before their reads. Strides count
        elements and byte_offset addresses the Region's logical bytes.
        """
        import torch

        if not self._handle:
            raise Invalid("Use has ended")
        code, bits, _ = dtype_descriptor(dtype)
        capsule = _ext._varena_public_use_dlpack(
            self._handle,
            _uint(index),
            code,
            bits,
            [operator.index(value) for value in shape],
            None if strides is None else [operator.index(value) for value in strides],
            _uint(byte_offset),
            _uint(self.stream if stream is None else stream),
        )
        return torch.utils.dlpack.from_dlpack(capsule)


class Allocation(_Owned):
    _close = "free"

    def __init__(self, handle: int, gpu: Gpu, stream: int) -> None:
        super().__init__(handle)
        self.gpu, self.stream = gpu, stream

    @property
    def ptr(self) -> int:
        return _new("allocation_ptr", self._handle)

    def record_stream(self, stream: int) -> None:
        _call("record_stream", self._handle, _uint(stream))


class CacheView:
    def __init__(self, handle: int, gpu: Gpu) -> None:
        self._handle, self.gpu = handle, gpu

    def info(self) -> dict[str, int]:
        value = _CacheInfo()
        _call("cache_info", self._handle, C.byref(value), C.sizeof(value))
        return _fields(value)

    def release(self, offset: int, bytes: int, stream: int) -> None:
        _call("cache_release", self._handle, _uint(offset), _uint(bytes), _uint(stream))


class Cache(_Owned, CacheView):
    _close = "cache_close"

    def __init__(self, handle: int, gpu: Gpu) -> None:
        _Owned.__init__(self, handle)
        self.gpu = gpu
        self._callback: Callable[[int], bool] | None = None
        self._owner: _ext._VarenaCacheOwner | None = None

    def close(self) -> None:
        # The ABI Cache retains/drains the same native weak callback owner, including
        # deferred finalization. Explicit close refusal keeps all Python owners here.
        super().close()
        self._owner = None
        self._callback = None

    def commit(self, offset: int, bytes: int) -> None:
        _call("cache_commit", self._handle, _uint(offset), _uint(bytes))


class Plan(_Owned):
    _close = "plan_close"

    def __init__(self, handle: int, gpu: Gpu) -> None:
        super().__init__(handle)
        self.gpu = gpu

    def activate(self, name: str) -> None:
        """Declare Region intent without a compute fingerprint or H observation."""
        _call("plan_activate", self._handle, _text(name))

    def deactivate(self) -> None:
        """Retire active intent; no execution completion is inferred."""
        _call("plan_deactivate", self._handle)

    @contextmanager
    def phase(self, name: str, execution_key: str) -> Iterator[None]:
        _call("phase_enter", self._handle, _text(name), _text(execution_key))
        try:
            yield
        except BaseException:
            _call("phase_leave", self._handle, 0)
            raise
        else:
            _call("phase_leave", self._handle, 1)

    def split(self, key: str, selection: SplitSelection, stream: int) -> Split:
        """Inside a phase: measure the level `Gpu.split_select` chose. Allocate it all on `stream`."""
        handle = _new("split_begin", self._handle, _text(key), _text(selection["id"]), _uint(stream), selection["generation"])
        return Split(self, handle)
