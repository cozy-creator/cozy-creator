"""Thin test control for the existing real C public-Source provider.

The shared helper owns its mapped HOST gates and retained CUDA context. This
module supplies no Source callback, producer/staging policy or allocator retry.
Unproven native owners are retained until their isolated proof child exits.
"""

from __future__ import annotations

import ctypes as C
import operator
import threading
from pathlib import Path

from tensorfs.varena import api

_MODES = {"ordering": 0, "partial": 1, "prepare_error": 2}
# An intentional native quarantine can outlive every local Python fixture name.
# Keep its callback code/control rooted independently of a collectable cycle.
_RETAINED: list[Provider] = []
_COUNTERS = (
    "prepares",
    "submits",
    "job_retired",
    "job_dropped",
    "source_retired",
    "source_dropped",
    "source_pending",
)


class _Counts(C.Structure):
    _fields_ = [(name, C.c_uint32) for name in _COUNTERS]


def _uint(value: int) -> int:
    result = operator.index(value)
    if not 0 <= result < 2**64:
        raise ValueError("expected an unsigned 64-bit fixture argument")
    return result


class Provider:
    """One matched helper library; at most one live Source fixture per process."""

    def __init__(self, path: str | Path):
        self.path = Path(path).resolve(strict=True)
        self._library = C.CDLL(str(self.path))
        for name, args, result in (
            ("fixture_abi", [], C.c_uint32),
            ("core_identity", [], C.c_uint64),
            ("last_error", [], C.c_char_p),
            ("host_contract", [], C.c_int32),
            (
                "create",
                [
                    C.c_uint64,
                    C.c_uint64,
                    C.c_int32,
                    C.POINTER(C.c_void_p),
                    C.POINTER(C.c_uint64),
                ],
                C.c_int32,
            ),
            ("gate_set", [C.c_void_p, C.c_uint32], C.c_int32),
            ("gate_wait", [C.c_void_p, C.c_uint64], C.c_int32),
            ("extra_gate_set", [C.c_void_p, C.c_uint32], C.c_int32),
            ("extra_gate_wait", [C.c_void_p, C.c_uint64], C.c_int32),
            ("job_retirement_hold", [C.c_void_p, C.c_uint32], C.c_int32),
            ("source_retirement_hold", [C.c_void_p, C.c_uint32], C.c_int32),
            (
                "job_retirement_pending",
                [C.c_void_p, C.POINTER(C.c_uint32)],
                C.c_int32,
            ),
            (
                "stream_pending",
                [C.c_void_p, C.c_uint64, C.POINTER(C.c_int32)],
                C.c_int32,
            ),
            ("source_pending", [C.c_void_p, C.POINTER(C.c_int32)], C.c_int32),
            ("counts", [C.c_void_p, C.POINTER(_Counts), C.c_size_t], C.c_int32),
            ("close", [C.c_void_p], C.c_int32),
        ):
            function = getattr(self._library, "te_" + name)
            function.argtypes, function.restype = args, result
        if self._library.te_fixture_abi() != 1:
            raise RuntimeError("Source fixture helper ABI mismatch")
        if self._library.te_core_identity() != api.core_identity():
            raise RuntimeError("Source fixture must share the installed native core")
        # This is a test ownership root, not automatic cleanup: a failed fixture
        # keeps the helper's code loaded through any native callback/quarantine.
        self._fixtures: list[Fixture] = []

    def _call(self, name: str, *args) -> None:
        status = getattr(self._library, "te_" + name)(*args)
        if status:
            detail = self._library.te_last_error()
            message = detail.decode("utf-8", "replace") if detail else name
            error = {
                1: api.Exhausted,
                2: api.Capturing,
                3: api.SourceError,
                4: api.Fault,
                5: api.Invalid,
                6: api.Unsupported,
                7: api.Busy,
            }.get(status, api.Error)
            raise error(message)

    def host_contract(self) -> None:
        """Run the original provider's real host-only invalid-constructor checks."""
        self._call("host_contract")

    def create(self, gpu: api.Gpu, nbytes: int, mode: str = "ordering") -> Fixture:
        """Adopt one existing C Source into the canonical public Region path.

        The caller installs Varena and explicitly selects the Torch device first.
        Both mapped HOST gates start closed. The Source producer waits on gate0;
        gate1 is independent and can prove unrelated work remains pending.
        """
        import torch

        if mode not in _MODES:
            raise ValueError(f"unknown Source fixture mode: {mode}")
        if not torch.cuda.is_initialized() or torch.cuda.current_device() != gpu.device:
            raise ValueError(
                "install Varena and select this explicit Torch device first"
            )
        handle, nbytes = _uint(gpu._handle), _uint(nbytes)
        pointer, region = C.c_void_p(), C.c_uint64()
        if self not in _RETAINED:
            # Even a failed C constructor may retain exact partial CUDA owners.
            # Such a failed isolated proof keeps this root until child exit.
            _RETAINED.append(self)
        self._call(
            "create",
            handle,
            nbytes,
            _MODES[mode],
            C.byref(pointer),
            C.byref(region),
        )
        fixture = Fixture(self, pointer, api.Region(region.value, gpu), mode)
        self._fixtures.append(fixture)
        return fixture


class Fixture:
    """Explicit owning test handle; close refuses unretired native resources.

    End public handles/aliases, open the gates and complete helper-gated streams
    before close. There is deliberately no finalizer freeing unproven owners.
    A capture-tail quarantine therefore keeps this exact helper until child exit.
    """

    def __init__(
        self, provider: Provider, pointer: C.c_void_p, region: api.Region, mode: str
    ):
        self._provider = provider
        self._pointer = pointer
        self._lock = threading.RLock()
        self.region = region
        self.mode = mode

    def _call(self, name: str, *args) -> None:
        if not self._pointer.value:
            raise api.Invalid("Source test fixture is closed")
        self._provider._call(name, self._pointer, *args)

    def gate_set(self, value: int) -> None:
        value = operator.index(value)
        if value not in (0, 1):
            raise ValueError("gate value must be zero or one")
        with self._lock:
            self._call("gate_set", value)

    def extra_gate_set(self, value: int) -> None:
        value = operator.index(value)
        if value not in (0, 1):
            raise ValueError("gate value must be zero or one")
        with self._lock:
            self._call("extra_gate_set", value)

    def gate_wait(self, stream: int) -> None:
        with self._lock:
            self._call("gate_wait", _uint(stream))

    def extra_gate_wait(self, stream: int) -> None:
        with self._lock:
            self._call("extra_gate_wait", _uint(stream))

    def stream_pending(self, stream: int) -> bool:
        value = C.c_int32()
        with self._lock:
            self._call("stream_pending", _uint(stream), C.byref(value))
        return bool(value.value)

    def source_pending(self) -> bool:
        """Query the actual producer stream while its native Job still owns it."""
        value = C.c_int32()
        with self._lock:
            self._call("source_pending", C.byref(value))
        return bool(value.value)

    def job_retirement_hold(self, hold: bool) -> None:
        """Hold/release real Job retirement; release on the same Python thread."""
        self._retirement_hold("job_retirement_hold", hold)

    def source_retirement_hold(self, hold: bool) -> None:
        """Hold the literal provider's Source.borrowed mutex on this thread."""
        self._retirement_hold("source_retirement_hold", hold)

    def _retirement_hold(self, name: str, hold: bool) -> None:
        value = operator.index(hold)
        if value not in (0, 1):
            raise ValueError("retirement hold must be zero or one")
        with self._lock:
            self._call(name, value)

    def job_retirement_pending(self) -> int:
        """Count actual VR_BUSY returns from the native Job retirement mutex."""
        value = C.c_uint32()
        with self._lock:
            self._call("job_retirement_pending", C.byref(value))
        return value.value

    def counts(self) -> dict[str, int]:
        value = _Counts()
        with self._lock:
            self._call("counts", C.byref(value), C.sizeof(value))
        return {name: getattr(value, name) for name in _COUNTERS}

    def close(self) -> None:
        with self._lock:
            self._call("close")
            self._pointer = C.c_void_p()
            self._provider._fixtures.remove(self)
            if not self._provider._fixtures:
                _RETAINED.remove(self._provider)
