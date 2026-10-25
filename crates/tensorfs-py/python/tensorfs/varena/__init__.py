"""Process-local CUDA weight reservations with ordinary owned Torch tensor views.

Installation preserves Torch's native caching allocator. Recovery covers its synchronous
cudaMalloc imports on supported Linux ELF builds, from all threads. cudaMallocAsync,
expandable segments, other CUDA libraries, and CUDA graph recovery are outside this contract.
"""

from __future__ import annotations

import os
from collections.abc import Iterator, Sequence
from contextlib import contextmanager

from .. import _ext

try:
    PoisonedError = _ext.VarenaPoisonedError
except AttributeError as error:
    raise RuntimeError(
        "TensorFS native Varena capability is missing: VarenaPoisonedError"
    ) from error

_installed = False


def install_allocator() -> None:
    """Install native allocation recovery before Torch reserves CUDA memory."""
    global _installed
    if _installed:
        return
    required = (
        "_VarenaArena",
        "_VarenaAllocatorCapture",
        "_varena_install_allocator",
        "_varena_set_activation_limit",
        "_varena_allocator_stats",
    )
    missing = [name for name in required if not hasattr(_ext, name)]
    if missing:
        raise RuntimeError(
            "TensorFS native Varena capability is missing: " + ", ".join(missing)
        )
    import torch

    if torch.cuda.get_allocator_backend() != "native":
        raise RuntimeError("Varena requires Torch's native CUDA caching allocator")
    config = os.environ.get(
        "PYTORCH_ALLOC_CONF", os.environ.get("PYTORCH_CUDA_ALLOC_CONF", "")
    )
    if "expandable_segments:true" in config.lower().replace(" ", ""):
        raise RuntimeError(
            "Varena's allocation hook does not cover expandable segments"
        )
    if torch.cuda.is_initialized() and any(
        torch.cuda.memory_reserved(i) for i in range(torch.cuda.device_count())
    ):
        raise RuntimeError("install Varena before Torch reserves CUDA memory")
    _ext._varena_install_allocator()
    _installed = True


def set_activation_limit(device: int, bytes: int | None = None) -> None:
    """Bound admission of hooked Torch segments plus current resident weights.

    Configure before Torch reserves memory. This is an activation-allocation admission
    limit, not a hard process/device cap: later weight fills, contexts, and unhooked CUDA
    allocations are not governed by it.
    """
    _ext._varena_set_activation_limit(device, bytes)


def allocator_stats(device: int = 0) -> dict[str, int]:
    return _ext._varena_allocator_stats(device)


class Arena:
    def __init__(self, device: int) -> None:
        self._native = _ext._VarenaArena(device)
        self.device = device

    @property
    def granularity(self) -> int:
        return self._native.granularity

    def reserve(self, bytes: int) -> Region:
        return Region(self._native.reserve(bytes))

    def set_streaming_limit(self, bytes: int) -> None:
        self._native.set_streaming_limit(bytes)

    def reclaim(self, bytes: int) -> int:
        return self._native.reclaim(bytes)

    def stats(self) -> dict[str, int]:
        return self._native.stats()

    @contextmanager
    def capture(self) -> Iterator[None]:
        """Forbid native recovery while dependencies are captured or replayed."""
        with capture(self.device):
            yield


class Region:
    def __init__(self, native: _ext._VarenaRegion) -> None:
        self._native = native

    @property
    def resident(self) -> bool:
        return self._native.resident

    @property
    def preferred(self) -> bool:
        return self._native.preferred

    @property
    def allocated_bytes(self) -> int:
        return self._native.allocated_bytes

    @property
    def ptr(self) -> int:
        return self._native.ptr

    def set_preferred(self, preferred: bool) -> None:
        self._native.set_preferred(preferred)

    def prepare(self) -> Use:
        return Use(self._native.prepare())

    def prepare_read(
        self,
        checkpoint: _ext.Checkpoint,
        parts: Sequence[tuple[str, int, int]],
        *,
        threads: int = 2,
        reclaim: bool = True,
    ) -> PreparedRead:
        """Admit a fill and warm bounded staging, without starting source I/O.

        Each part is (component/key#role, destination byte offset, exact stored length).
        This region must be dedicated to that immutable source recipe. Native prefetch
        uses local available sources only; it never calls a Python transformer/fetcher.
        reclaim=False refuses admission when eviction or a prior-writer join is needed.
        """
        native = getattr(_ext, "_VarenaPreparedRead", None)
        if native is None:
            raise RuntimeError(
                "this TensorFS build has no native owned refill capability"
            )
        return PreparedRead(native(self._native, checkpoint, list(parts), threads, reclaim))


def supports_refill() -> bool:
    """Whether this prebuilt extension exposes the owning native refill interface."""
    return hasattr(_ext, "_VarenaPreparedRead")


def supports_prefetch() -> bool:
    """Whether native refill can refuse admission without eviction or a context join."""
    return bool(getattr(getattr(_ext, "_VarenaPreparedRead", None), "NO_RECLAIM", False))


class PreparedRead:
    """Own source/staging/Fill until checked producer completion, without a context join.

    Call start after active work is admitted. A successful wait/poll publishes only
    completed bytes, so ordinary Region.prepare/DLPack exports remain stream-safe.
    cancel() requests stop; wait() or completed poll() proves the submitted work drained.
    An unstarted cancellation settles without a later start. Partial fills are never published.
    """

    def __init__(self, native: _ext._VarenaPreparedRead) -> None:
        self._native = native

    def start(self) -> None:
        self._native.start()

    def cancel(self) -> None:
        self._native.cancel()

    def poll(self) -> dict[str, int | bool]:
        return self._native.poll()

    def wait(self) -> dict[str, int | bool]:
        return self._native.wait()


class Use:
    """Pinned storage. Fill every byte before commit when needs_fill is true.

    The tensor and all its aliases retain their own native physical pin, even after this
    object is destroyed. An abandoned fill stays unreadable to later prepares.
    """

    def __init__(self, native: _ext._VarenaUse) -> None:
        import torch

        self._native = native
        self.needs_fill = native.needs_fill
        self.tensor = torch.from_dlpack(native)

    def commit(self) -> None:
        self._native.commit()
        self.needs_fill = False


@contextmanager
def capture(device: int) -> Iterator[None]:
    """Inhibit allocation recovery on this device across every native thread."""
    guard = _ext._VarenaAllocatorCapture(device)
    try:
        yield
    finally:
        guard.close()
