"""Whole-process Torch working allocator. Install before CUDA initialization in the executor child.

Use this module's graph/replay owners, or explicitly complete capture and record every replay
and output consumer stream. Raw Torch graph calls alone do not supply these lifetime signals.
"""
from __future__ import annotations

import ctypes
from contextlib import contextmanager
import json
import os
from pathlib import Path
import platform
import sys
import threading
from types import ModuleType
from typing import TYPE_CHECKING, Any, Iterator, Literal, Sequence, TypeAlias, cast

from .. import _ext

if TYPE_CHECKING:
    from torch import device as TorchDevice
    from torch.cuda import Stream

DeviceLike: TypeAlias = "int | str | TorchDevice | None"
PoolId: TypeAlias = tuple[int, int]

_lock = threading.Lock()
_owner = 0
_library: ctypes.CDLL | None = None
_allocator: Any = None
_CAPTURE_CAPACITY_PREFIX = "[tensorfs.varena.capture_capacity.v1] "


def _installed() -> None:
    if _owner != os.getpid():
        raise RuntimeError("install tensorfs.varena.torch in this child before using its allocator")


def _device(device: DeviceLike = None) -> int:
    import torch

    if device is None:
        return cast(int, torch.cuda.current_device())
    if isinstance(device, int):
        if device < 0:
            raise ValueError("device must be nonnegative")
        return device
    parsed = torch.device(device)
    if parsed.type != "cuda":
        raise ValueError("Varena requires a CUDA device")
    return cast(int, torch.cuda.current_device() if parsed.index is None else parsed.index)


def install(device: DeviceLike = None) -> None:
    """Select Varena once per process. An explicit device is initialized outside capture.

    The wheel must list this Torch patch cohort and match its CUDA/C++ ABI and platform.
    This never compiles at runtime and uses no cudaMalloc import hook.
    """
    global _owner, _library, _allocator
    import torch

    if _owner and _owner != os.getpid():
        _installed()  # Reject before touching a potentially inherited installation lock.
    with _lock:
        if _owner:
            _installed()
        else:
            if torch.cuda.is_initialized():
                raise RuntimeError("install Varena before Torch CUDA lazy initialization")
            if torch.backends.cudnn.benchmark:
                raise RuntimeError("Varena requires cudnn.benchmark=False")
            directory = Path(__file__).resolve().parents[1]
            library = directory / "_varena_torch.so"
            metadata = library.with_suffix(".json")
            if not library.is_file() or not metadata.is_file():
                raise RuntimeError("this TensorFS wheel has no Torch shim; build with scripts/build-wheel.sh --torch-python <python-with-torch>")
            actual = json.loads(metadata.read_text())
            runtime_torch = str(torch.__version__)
            expected = {
                "abi": 1, "cuda": torch.version.cuda,
                "cxx11_abi": int(torch._C._GLIBCXX_USE_CXX11_ABI),
                "system": platform.system(), "machine": platform.machine(),
            }
            if runtime_torch not in actual.get("supported_torch", ()) or any(actual.get(key) != value for key, value in expected.items()):
                raise RuntimeError(f"Varena Torch shim ABI mismatch: built {actual}, running {dict(torch=runtime_torch, **expected)}")
            native = ctypes.CDLL(str(library))
            native.varena_torch_bind.argtypes = [ctypes.c_void_p]
            native.varena_torch_bind.restype = ctypes.c_int
            native.varena_torch_abort_routing.argtypes = [ctypes.c_int, ctypes.c_uint64, ctypes.c_uint64]
            native.varena_torch_abort_routing.restype = ctypes.c_int
            bindings = _ext._varena_working_bindings()
            if native.varena_torch_bind(bindings) != 0:
                raise RuntimeError("Varena Torch shim refused its native function table")

            def address(name: str) -> int:
                return cast(int, ctypes.cast(getattr(native, name), ctypes.c_void_p).value)

            allocator = torch.cuda.memory.CUDAPluggableAllocator(
                str(library), "varena_torch_malloc", "varena_torch_free"
            )
            hooks = allocator.allocator()
            for setter, symbol in (
                ("set_record_stream_fn", "varena_torch_record_stream"),
                ("set_reset_fn", "varena_torch_reset"),
                ("set_begin_allocate_to_pool", "varena_torch_begin_pool"),
                ("set_end_allocate_to_pool_fn", "varena_torch_end_pool"),
                ("set_release_pool", "varena_torch_release_pool"),
            ):
                getattr(hooks, setter)(address(symbol))
            torch.cuda.memory.change_current_allocator(allocator)
            _library, _allocator, _owner = native, allocator, os.getpid()
            _bind_stats(torch)
    if device is not None:
        initialize(device)


def initialize(device: DeviceLike = 0) -> None:
    """Prepare a device outside capture, including an allocation-free first graph."""
    _installed()
    _ext._varena_working_initialize(_device(device))


def stats(device: DeviceLike = 0) -> dict[str, int]:
    _installed()
    return _ext._varena_working_stats(_device(device))


def reset_peak(device: DeviceLike = 0) -> None:
    """Reset measured working peaks to current live and mapped bytes."""
    _installed()
    _ext._varena_working_reset_peak(_device(device))


def trim(device: DeviceLike = 0) -> int:
    """Return completed unused pages; pending work and graph owners remain retained."""
    _installed()
    return _ext._varena_working_trim(_device(device))


def set_limit(device: DeviceLike, bytes: int | None = None) -> int:
    """Bound resident weights plus mapped Torch/native working memory.

    This static ceiling is independent of temporary make_room limits; the smaller wins.
    Contexts and allocations owned by other CUDA libraries are outside this managed total.
    Live owners are retained when lowering the limit. Returns unused bytes reclaimed now.
    """
    _installed()
    return _ext._varena_working_set_limit(_device(device), bytes)


def split_context(device: int) -> dict[str, Any]:
    """Metadata-only eligibility for an explicit device; no implicit CUDA initialization."""
    _installed()
    if device < 0:
        raise ValueError("device must be nonnegative")
    return _ext._varena_split_context(device)


def split_select(device: DeviceLike, key: str, choices: Sequence[tuple[str, Sequence[int]]], *, preferred: str, stream: int) -> dict[str, Any]:
    """Choose an opaque legal variant from a complete copied allocation menu before execution."""
    _installed()
    return _ext._varena_split_select(_device(device), key, [(name, list(sizes)) for name, sizes in choices], preferred=preferred, stream=stream)


def split_begin(device: DeviceLike, key: str, choice_id: str, *, stream: int, completion_owner: int | None = None) -> int:
    """Commit exactly this choice and open one nonnested leaf interval within its phase.

    completion_owner is the active eager observation whose checked stream completion also
    covers this leaf. None permits declared selection but cannot train a successful sample.
    """
    _installed()
    return _ext._varena_split_begin(_device(device), key, choice_id, stream=stream, completion_owner=completion_owner)


def split_end(device: DeviceLike, observation_id: int, *, submitted: bool) -> dict[str, Any]:
    """Close host submission; only the enclosing phase's checked completion can publish."""
    _installed()
    return _ext._varena_split_end(_device(device), observation_id, submitted=submitted)


def phase_enter(device: DeviceLike, key: str | None = None, *, mode: Literal["eager", "capture", "replay", "preparation"] = "eager") -> int:
    """Begin one execution observation. An unknown key never enables speculation.

    A known key promises fixed geometry/dispatch/split choices. Only successful eager
    execution can publish in this checkpoint; recording and replay remain unqualified.
    """
    _installed()
    return _ext._varena_phase_enter(_device(device), key, mode=mode)


def phase_leave(device: DeviceLike, token: int, *, completed: bool, key: str | None = None) -> dict[str, Any]:
    """End after checked execution completion, supplying an actually observed key if needed.

    Aborted, retried, recording/preparation/replay and mismatched-key observations never
    replace the previous successful charged-capacity sample. This performs no CUDA work.
    """
    _installed()
    return _ext._varena_phase_leave(_device(device), token, completed=completed, key=key)


def phase_invalidate(device: DeviceLike, token: int, reason: str = "execution_retry") -> None:
    """Invalidate a caught failed dispatch/retry without hiding its original exception."""
    _installed()
    _ext._varena_phase_invalidate(_device(device), token, reason)


def phase_stats(device: DeviceLike, key: str) -> dict[str, Any]:
    """Owned block/backing peaks and total-process telemetry, with separate aborted counts."""
    _installed()
    return _ext._varena_phase_stats(_device(device), key)


def phase_forget(device: DeviceLike, key: str) -> bool:
    """Retire a dead model/plan's measurement after its final execution owner releases it."""
    _installed()
    return _ext._varena_phase_forget(_device(device), key)


def prepare_pool(pool: PoolId, bytes: int, device: DeviceLike = 0) -> int:
    """Map at least this capacity before capture; the previous pool survives failure."""
    initialize(device)
    return _ext._varena_working_pool_reserve(_device(device), cast(PoolId, tuple(pool)), bytes)


def cancel_preparation(pool: PoolId, device: DeviceLike = 0) -> None:
    """Return an unused reservation to reclaimability without releasing a graph owner."""
    _installed()
    _ext._varena_working_pool_cancel_preparation(_device(device), cast(PoolId, tuple(pool)))


def capture_complete(pool: PoolId, *, stream: Stream, device: DeviceLike = None) -> None:
    """Release capture exclusion after the owner actually ended this stream's capture."""
    _installed()
    import torch

    index = _device(stream.device if device is None else device)
    with torch.cuda.device(index), torch.cuda.stream(stream):
        if torch.cuda.is_current_stream_capturing():
            raise RuntimeError("end CUDA stream capture before completing its Varena pool")
        _ext._varena_working_capture_complete(index, cast(PoolId, tuple(pool)))


def record_pool_stream(pool: PoolId, stream: Stream, device: DeviceLike = None) -> None:
    """Record after replay or output use has been submitted on this stream."""
    _installed()
    index = _device(stream.device if device is None else device)
    _ext._varena_working_pool_record_stream(index, cast(PoolId, tuple(pool)), stream.cuda_stream)


def retire_stream(stream: Stream, device: DeviceLike = None) -> None:
    """Forget a drained stream before its owner destroys it; the caller stops submissions first."""
    _installed()
    index = _device(stream.device if device is None else device)
    _ext._varena_working_retire_stream(index, stream.cuda_stream)


@contextmanager
def external_callback(device: DeviceLike) -> Iterator[None]:
    """Mark only actual callback-interval alloc/free as unrepresented execution demand.

    This per-device interval conservatively includes concurrent worker allocations.
    Detached work after callback return is outside the callback's owned lifetime.
    """
    _installed()
    scope = _ext._VarenaCallbackScope(_device(device))
    try:
        yield
    finally:
        scope.close()


def capture_footprint(device: DeviceLike, pool: PoolId) -> _ext._VarenaGraphFootprint:
    """Create inert graph-definition metadata; pass it to graph(..., footprint=...)."""
    _installed()
    return _ext._VarenaGraphFootprint(_device(device), pool)


@contextmanager
def graph(cuda_graph: Any, *, pool: PoolId | None = None, stream: Stream | None = None, reserve_bytes: int | None = None, footprint: _ext._VarenaGraphFootprint | None = None) -> Iterator[Any]:
    """Prepare, capture and finish one graph. Shared-pool replay order remains the owner's contract."""
    _installed()
    import torch

    device = _device(stream.device if stream is not None else None)
    stream = torch.cuda.Stream(device=device) if stream is None else stream
    pool = cast(PoolId, torch.cuda.graph_pool_handle()) if pool is None else pool
    try:
        prepare_pool(pool, stats(device)["allocated_peak"] if reserve_bytes is None else reserve_bytes, device)
        # Preparation is outside capture; native allocations during capture use this pool only.
        torch.cuda.synchronize(device)
    except BaseException:
        cancel_preparation(pool, device)
        raise
    started = False
    error = None
    tracing = False
    try:
        with torch.cuda.device(device), torch.cuda.stream(stream):
            try:
                if footprint is not None:
                    # Binding is to this capture cycle, not merely the shared pool. The callback
                    # references only metadata and makes no CUDA call on reset or finalization.
                    cuda_graph.register_destroy_callback(footprint.retire)
                    footprint._begin(device, pool)
                    tracing = True
                cuda_graph.capture_begin(pool=pool)
                started = True
                yield cuda_graph
            except BaseException as caught:
                error = caught
            finally:
                # A capture-start Python hook may throw after CUDA capture already began.
                with torch.cuda.stream(stream):
                    ending = started or torch.cuda.is_current_stream_capturing()
                    if ending:
                        try:
                            cuda_graph.capture_end()
                        except BaseException as caught:
                            if error is None:
                                error = caught
                    if torch.cuda.is_current_stream_capturing():
                        raise RuntimeError("CUDA capture did not end; Varena capture exclusion is retained") from error
                    routed = cast(ctypes.CDLL, _library).varena_torch_abort_routing(device, *pool)
                    if routed < 0:
                        raise RuntimeError("failed to end Varena capture routing") from error
                    if ending or routed:
                        capture_complete(pool, stream=stream, device=device)
                        if error is not None:
                            cuda_graph.reset()
                    elif error is not None:
                        cancel_preparation(pool, device)
    finally:
        if tracing and footprint is not None:
            footprint._finish(error is None and sys.exc_info()[0] is None)
    if error is not None:
        # This private c10::Error escapes cuDNN's engine-fallback catches. Publish OOM only
        # after actual capture completion and graph reset; cleanup failures take precedence.
        if (
            type(error) is RuntimeError
            and len(error.args) == 1
            and isinstance(error.args[0], str)
            and error.args[0].startswith(_CAPTURE_CAPACITY_PREFIX)
        ):
            detail = error.args[0][len(_CAPTURE_CAPACITY_PREFIX):]
            raise torch.OutOfMemoryError(detail) from error
        raise error.with_traceback(error.__traceback__)
    cuda_graph._varena_owner = (_owner, device, cast(PoolId, tuple(pool)))


def replay(cuda_graph: Any, *, stream: Stream | None = None) -> None:
    """Replay an owned graph and retain its pool through completion on the selected stream."""
    _installed()
    import torch

    owner, device, pool = cuda_graph._varena_owner
    if owner != _owner:
        raise RuntimeError("a CUDA graph cannot be replayed after fork")
    stream = torch.cuda.current_stream(device) if stream is None else stream
    if _device(stream.device) != device:
        raise ValueError("replay stream belongs to a different device")
    _ext._varena_working_pool_retain(device, pool)
    try:
        with torch.cuda.device(device), torch.cuda.stream(stream):
            try:
                cuda_graph.replay()
            finally:
                # End hooks may raise or reset the graph after launch. Our reference survives both.
                record_pool_stream(pool, stream, device)
    finally:
        _ext._varena_working_pool_release(device, pool)


def _bind_stats(torch: ModuleType) -> None:
    def memory_stats(device: DeviceLike = None) -> dict[str, int]:
        row = stats(_device(device))
        return {
            "allocated_bytes.all.current": row["allocated_bytes"],
            "allocated_bytes.all.peak": row["allocated_peak"],
            "reserved_bytes.all.current": row["mapped_bytes"],
            "reserved_bytes.all.peak": row["mapped_peak"],
            "allocation.all.current": row["allocations"] - row["frees"],
            "num_ooms": row["ooms"],
        }

    def reset_peak_memory_stats(device: DeviceLike = None) -> None:
        reset_peak(_device(device))

    readers = {
        "memory_stats": memory_stats,
        "memory_allocated": lambda device=None: stats(_device(device))["allocated_bytes"],
        "memory_reserved": lambda device=None: stats(_device(device))["mapped_bytes"],
        "max_memory_allocated": lambda device=None: stats(_device(device))["allocated_peak"],
        "max_memory_reserved": lambda device=None: stats(_device(device))["mapped_peak"],
        "reset_peak_memory_stats": reset_peak_memory_stats,
    }
    for module in (torch.cuda, torch.cuda.memory):
        for name, function in readers.items():
            setattr(module, name, function)
