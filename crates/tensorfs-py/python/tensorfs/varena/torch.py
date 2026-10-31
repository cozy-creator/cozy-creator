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
import threading
from typing import Iterator

from .. import _ext

_lock = threading.Lock()
_owner = 0
_library = None
_allocator = None
_CAPTURE_CAPACITY_PREFIX = "[tensorfs.varena.capture_capacity.v1] "


def _installed() -> None:
    if _owner != os.getpid():
        raise RuntimeError("install tensorfs.varena.torch in this child before using its allocator")


def _device(device=None) -> int:
    import torch

    if device is None:
        return torch.cuda.current_device()
    if isinstance(device, int):
        if device < 0:
            raise ValueError("device must be nonnegative")
        return device
    parsed = torch.device(device)
    if parsed.type != "cuda":
        raise ValueError("Varena requires a CUDA device")
    return torch.cuda.current_device() if parsed.index is None else parsed.index


def install(device: int | None = None) -> None:
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
                return ctypes.cast(getattr(native, name), ctypes.c_void_p).value

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


def initialize(device: int = 0) -> None:
    """Prepare a device outside capture, including an allocation-free first graph."""
    _installed()
    _ext._varena_working_initialize(_device(device))


def stats(device: int = 0) -> dict[str, int]:
    _installed()
    return _ext._varena_working_stats(_device(device))


def reset_peak(device: int = 0) -> None:
    """Reset measured working peaks to current live and mapped bytes."""
    _installed()
    _ext._varena_working_reset_peak(_device(device))


def trim(device: int = 0) -> int:
    """Return completed unused pages; pending work and graph owners remain retained."""
    _installed()
    return _ext._varena_working_trim(_device(device))


def set_limit(device: int, bytes: int | None = None) -> int:
    """Bound resident weights plus mapped Torch/native working memory.

    This static ceiling is independent of temporary make_room limits; the smaller wins.
    Contexts and allocations owned by other CUDA libraries are outside this managed total.
    Live owners are retained when lowering the limit. Returns unused bytes reclaimed now.
    """
    _installed()
    return _ext._varena_working_set_limit(_device(device), bytes)


def prepare_pool(pool: tuple[int, int], bytes: int, device: int = 0) -> int:
    """Map at least this capacity before capture; the previous pool survives failure."""
    initialize(device)
    return _ext._varena_working_pool_reserve(_device(device), tuple(pool), bytes)


def cancel_preparation(pool: tuple[int, int], device: int = 0) -> None:
    """Return an unused reservation to reclaimability without releasing a graph owner."""
    _installed()
    _ext._varena_working_pool_cancel_preparation(_device(device), tuple(pool))


def capture_complete(pool: tuple[int, int], *, stream, device: int | None = None) -> None:
    """Release capture exclusion after the owner actually ended this stream's capture."""
    _installed()
    import torch

    index = _device(stream.device if device is None else device)
    with torch.cuda.device(index), torch.cuda.stream(stream):
        if torch.cuda.is_current_stream_capturing():
            raise RuntimeError("end CUDA stream capture before completing its Varena pool")
        _ext._varena_working_capture_complete(index, tuple(pool))


def record_pool_stream(pool: tuple[int, int], stream, device: int | None = None) -> None:
    """Record after replay or output use has been submitted on this stream."""
    _installed()
    index = _device(stream.device if device is None else device)
    _ext._varena_working_pool_record_stream(index, tuple(pool), stream.cuda_stream)


def retire_stream(stream, device: int | None = None) -> None:
    """Forget a drained stream before its owner destroys it; the caller stops submissions first."""
    _installed()
    index = _device(stream.device if device is None else device)
    _ext._varena_working_retire_stream(index, stream.cuda_stream)


@contextmanager
def graph(cuda_graph, *, pool=None, stream=None, reserve_bytes: int | None = None) -> Iterator[object]:
    """Prepare, capture and finish one graph. Shared-pool replay order remains the owner's contract."""
    _installed()
    import torch

    device = _device(stream.device if stream is not None else None)
    stream = torch.cuda.Stream(device=device) if stream is None else stream
    pool = torch.cuda.graph_pool_handle() if pool is None else pool
    try:
        prepare_pool(pool, stats(device)["allocated_peak"] if reserve_bytes is None else reserve_bytes, device)
        # Preparation is outside capture; native allocations during capture use this pool only.
        torch.cuda.synchronize(device)
    except BaseException:
        cancel_preparation(pool, device)
        raise
    started = False
    error = None
    with torch.cuda.device(device), torch.cuda.stream(stream):
        try:
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
                routed = _library.varena_torch_abort_routing(device, *pool)
                if routed < 0:
                    raise RuntimeError("failed to end Varena capture routing") from error
                if ending or routed:
                    capture_complete(pool, stream=stream, device=device)
                    if error is not None:
                        cuda_graph.reset()
                elif error is not None:
                    cancel_preparation(pool, device)
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
    cuda_graph._varena_owner = (_owner, device, tuple(pool))


def replay(cuda_graph, *, stream=None) -> None:
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


def _bind_stats(torch) -> None:
    def memory_stats(device=None):
        row = stats(_device(device))
        return {
            "allocated_bytes.all.current": row["allocated_bytes"],
            "allocated_bytes.all.peak": row["allocated_peak"],
            "reserved_bytes.all.current": row["mapped_bytes"],
            "reserved_bytes.all.peak": row["mapped_peak"],
            "allocation.all.current": row["allocations"] - row["frees"],
            "num_ooms": row["ooms"],
        }

    def reset_peak_memory_stats(device=None):
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
