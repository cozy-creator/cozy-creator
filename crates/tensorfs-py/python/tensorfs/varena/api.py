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
from itertools import count

from .. import _ext


class Error(RuntimeError):
    def __init__(self, message: str, requested_bytes: int = 0) -> None:
        super().__init__(message)
        self.requested_bytes = requested_bytes


class Exhausted(Error):
    pass


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


class _Phase(C.Structure):
    _fields_ = [("name", C.c_char_p), ("regions", _h), ("count", C.c_size_t)]


# Symbol lookup traverses _ext's DT_NEEDED dependencies. Never dlopen a second
# allocator by an unrelated absolute path, or statically embed a core in this module.
_lib = C.CDLL(_ext.__file__)


def _declare(name: str, args: list, result=C.c_int32):
    function = getattr(_lib, "vr_v1_" + name)
    function.argtypes = args
    function.restype = result
    return function


_declare("abi_version", [], C.c_uint32)
_declare("core_identity", [], _u)
_declare("last_error", [], C.c_char_p)
_declare("error_requested_bytes", [], _u)
for _name, _args in {
    "gpu_open": [C.c_int32, _h],
    "gpu_close": [_u],
    "region_from_host": [_u, C.c_void_p, C.c_size_t, _h],
    "region_ptr": [_u, _h],
    "region_close": [_u],
    "use": [_u, _h, C.c_size_t, _u, _h],
    "use_ptr": [_u, C.c_size_t, _h],
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
    "phase_enter": [_u, C.c_char_p, C.c_char_p],
    "phase_leave": [_u, C.c_int32],
    "plan_close": [_u],
    "stats": [_u, C.POINTER(_Stats), C.c_size_t],
    "trim": [_u, _h],
}.items():
    _declare(_name, _args)

if (
    _lib.vr_v1_abi_version() != 1
    or _lib.vr_v1_core_identity() != _ext._varena_core_identity()
):
    raise ImportError(
        "TensorFS and public Varena must resolve the same ABI 1 allocator core"
    )


def core_identity() -> int:
    return _lib.vr_v1_core_identity()


def _call(name: str, *args) -> None:
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


def _new(name: str, *args) -> int:
    out = _u()
    _call(name, *args, C.byref(out))
    return out.value


def _fields(value: C.Structure) -> dict[str, int]:
    return {name: getattr(value, name) for name, _ in value._fields_}


class _Owned:
    _close: str

    def __init__(self, handle: int) -> None:
        self._handle = handle

    def close(self) -> None:
        if self._handle:
            _call(self._close, self._handle)
            self._handle = 0

    def __enter__(self):
        if not self._handle:
            raise Invalid("handle is closed")
        return self

    def __exit__(self, *exc) -> None:
        self.close()

    def __del__(self) -> None:
        # Native failure retains the exact owner/backing, never invents completion.
        with suppress(Error, AttributeError):
            self.close()


class Gpu(_Owned):
    _close = "gpu_close"

    def __init__(self, device: int = 0) -> None:
        device = operator.index(device)
        if not 0 <= device < 2**31:
            raise ValueError("device must be a nonnegative int32")
        super().__init__(_new("gpu_open", device))
        self.device = device

    def region(self, data: bytes | bytearray | memoryview) -> Region:
        data = bytes(data)
        return Region(_new("region_from_host", self._handle, data, len(data)), self)

    def use(self, regions: Sequence[Region], stream: int = 0) -> Use:
        """Side-stream consumers must join stream before this Use closes."""
        ids = (_u * len(regions))(*(region._handle for region in regions))
        return Use(
            _new("use", self._handle, ids, len(ids), _uint(stream)),
            self,
            list(regions),
            stream,
        )

    def alloc(self, bytes: int, stream: int = 0) -> Allocation:
        return Allocation(
            _new("alloc", self._handle, _uint(bytes), _uint(stream)), self, stream
        )

    def cache(
        self, reserve: int, shrink: Callable[[CacheView, int], bool] | None = None
    ) -> Cache:
        reserve = _uint(reserve)
        if shrink is None:
            descriptor = _Shrinker()
            return Cache(_new("cache_new", self._handle, reserve, descriptor), self)
        # C owns this callback until its native retirement calls _drop. No bound
        # method of Cache is retained here; closures should not capture their Cache.
        context = next(_callback_ids)
        _callbacks[context] = (shrink, self)
        descriptor = _Shrinker(_shrink, _drop, context)
        try:
            handle = _new("cache_new", self._handle, reserve, descriptor)
        except BaseException:
            _callbacks.pop(context, None)
            raise
        return Cache(handle, self)

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

    def trim(self) -> int:
        return _new("trim", self._handle)


def open(device: int = 0) -> Gpu:
    return Gpu(device)


class Region(_Owned):
    _close = "region_close"

    def __init__(self, handle: int, gpu: Gpu) -> None:
        super().__init__(handle)
        self.gpu = gpu


class Use(_Owned):
    _close = "done"

    def __init__(
        self, handle: int, gpu: Gpu, regions: list[Region], stream: int
    ) -> None:
        super().__init__(handle)
        self.gpu, self.regions, self.stream = gpu, regions, stream

    def ptr(self, index: int = 0) -> int:
        if not self._handle:
            raise Invalid("Use has ended")
        return _new("use_ptr", self._handle, _uint(index))


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

    def release(self, offset: int, bytes: int, stream: int = 0) -> None:
        _call("cache_release", self._handle, _uint(offset), _uint(bytes), _uint(stream))


class Cache(_Owned, CacheView):
    _close = "cache_close"

    def __init__(self, handle: int, gpu: Gpu) -> None:
        _Owned.__init__(self, handle)
        self.gpu = gpu

    def commit(self, offset: int, bytes: int) -> None:
        _call("cache_commit", self._handle, _uint(offset), _uint(bytes))


class Plan(_Owned):
    _close = "plan_close"

    def __init__(self, handle: int, gpu: Gpu) -> None:
        super().__init__(handle)
        self.gpu = gpu

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


_callback_ids = count(1)
_callbacks: dict[int, tuple[Callable[[CacheView, int], bool], Gpu]] = {}


@_shrink_type
def _shrink(context: int, handle: int, bytes: int) -> int:
    try:
        callback, gpu = _callbacks[context]
        return 0 if callback(CacheView(handle, gpu), bytes) else 7
    except BaseException:  # noqa: BLE001 - exceptions must never escape a C callback
        return 3


@_drop_type
def _drop(context: int) -> None:
    _callbacks.pop(context, None)
