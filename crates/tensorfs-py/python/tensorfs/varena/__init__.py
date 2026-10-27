"""Process-local CUDA weight reservations with ordinary owned Torch tensor views.

Installation preserves Torch's native caching allocator. Recovery covers its synchronous
cudaMalloc imports on supported Linux ELF builds, from all threads. cudaMallocAsync,
expandable segments, other CUDA libraries, and CUDA graph recovery are outside this contract.
"""

from __future__ import annotations

import os
from collections.abc import Iterator, Sequence
from contextlib import contextmanager
from typing import TYPE_CHECKING

from .. import _ext

if TYPE_CHECKING:
    from torch import Tensor

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

    @property
    def streaming_limit_bytes(self) -> int:
        return self._native.streaming_limit_bytes

    def reclaim(self, bytes: int) -> int:
        return self._native.reclaim(bytes)

    def reset_preferences(self) -> None:
        """Begin an explicit execution phase without changing pins, mappings or VA."""
        self._native.reset_preferences()

    def reclaim_unpreferred(self, bytes: int) -> int:
        """Reclaim only idle nonpreferred backing; every physical pin still wins."""
        return self._native.reclaim_unpreferred(bytes)

    def stats(self) -> dict[str, int]:
        return self._native.stats()

    @contextmanager
    def capture(self) -> Iterator[None]:
        """Exclude native producers and recovery during actual CUDA capture."""
        with capture(self.device):
            yield


class PhaseController:
    """Internal per-submission phase decoration using native continuation ownership."""

    def __init__(self, arena: Arena) -> None:
        self._native = _ext._VarenaPhaseController(arena._native)

    def wrap(self, capsule, key: str, regions: Sequence[Region]):
        return self._native.wrap(capsule, key, [region._native for region in regions])

    def facts(self) -> dict[str, object]:
        return self._native.facts()


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

    def pin(self) -> Use | None:
        """Pin existing ready contents atomically, or return None for a residency miss.

        Never allocates, maps, refills or starts recovery. Errors such as poison and
        use after fork propagate. The returned Use and its aliases own the same physical
        pin as prepare(); this adds no ordering for writes after the committed fill.
        """
        native = self._native.pin()
        return None if native is None else Use(native)

    def prepare_read(
        self,
        checkpoint: _ext.Checkpoint,
        parts: Sequence[tuple[str, int, int]],
        *,
        threads: int = 2,
        reclaim: bool = True,
        evict_completed: bool = False,
    ) -> PreparedRead:
        """Admit a fill and warm bounded staging, without starting source I/O.

        Each part is (component/key#role, destination byte offset, exact stored length).
        This region must be dedicated to that immutable source recipe. Native prefetch
        uses local available sources only; it never calls a Python transformer/fetcher.
        reclaim=False refuses admission when eviction or a prior-writer join is needed;
        with evict_completed it may still evict idle streamed regions whose last-consumer
        events are complete, oldest first, never waiting.
        """
        native = getattr(_ext, "_VarenaPreparedRead", None)
        if native is None:
            raise RuntimeError(
                "this TensorFS build has no native owned refill capability"
            )
        return PreparedRead(
            native(self._native, checkpoint, list(parts), threads, reclaim, evict_completed)
        )


def supports_refill() -> bool:
    """Whether this prebuilt extension exposes the owning native refill interface."""
    return hasattr(_ext, "_VarenaPreparedRead")


def supports_phases() -> bool:
    """Whether this native build supports explicit metadata-only cache reprioritization."""
    return all(
        hasattr(_ext._VarenaArena, name)
        for name in ("reset_preferences", "reclaim_unpreferred")
    )


def supports_prefetch() -> bool:
    """Whether native refill can refuse admission without eviction or a context join."""
    native = getattr(_ext, "_VarenaPreparedRead", None)
    return bool(
        getattr(native, "NO_RECLAIM", False)
        and getattr(native, "CAPTURE_GUARD", False)
        and getattr(_ext._VarenaAllocatorCapture, "REPLAY_GUARD", False)
    )


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

    def consume_on(self, stream: int) -> None:
        """Declare a CUDA stream (``torch.cuda.Stream.cuda_stream``) that uses this storage.

        Once any stream is declared, releasing this Use or the last alias of its tensor records
        a Varena event on every declared stream, and reclaim waits on those events instead of
        joining the CUDA context. Declare every stream that touches this storage, including
        through escaped aliases, before that last alias is released.
        """
        self._native.consume_on(stream)


@contextmanager
def capture(device: int) -> Iterator[None]:
    """Exclude native producers and allocation recovery during actual CUDA capture."""
    guard = _ext._VarenaAllocatorCapture(device)
    try:
        yield
    finally:
        guard.close()


@contextmanager
def replay(device: int) -> Iterator[None]:
    """Suppress native allocation recovery while allowing independent owned producers.

    Retained storage pins protect the graph through device completion. This short guard
    protects the host replay call; it does not replace those owners.
    """
    guard = _ext._VarenaAllocatorCapture(device, replay=True)
    try:
        yield
    finally:
        guard.close()


class LoadSlot:
    """One initialized capture slot for a raw checkpoint load followed by native execution.

    This is dedicated storage, not a readable cache entry before load completion. Use its
    zero-initialized ``tensor`` only to prepare/capture the known body that will consume the
    stored parts. ``PreparedRepeat.capture(load=slot)`` seals that capture. Submission is
    one-shot; no Python callback runs between the native read and the retained continuation.
    Ordinary region prefetch keeps its existing behavior. ``encoded`` adds native decode
    before the same continuation. With matching capabilities, Runtime may select event
    ordering: producer events permit compute without a host completion wait, and a device
    input guard skips the complete authored program for invalid decoded values. Checked
    completion still precedes publication; late device/ownership failures discard outputs.
    """

    supports_event_input = bool(
        getattr(getattr(_ext, "_VarenaLoadSlot", None), "supports_event_input", False)
    )
    supports_region_input = hasattr(getattr(_ext, "_VarenaLoadSlot", None), "from_regions")
    supports_ready_stage = bool(
        getattr(getattr(_ext, "_VarenaLoadSlot", None), "supports_ready_stage", False)
    )

    def __init__(
        self,
        arena: Arena,
        checkpoint: _ext.Checkpoint,
        parts: Sequence[tuple[str, int, int]],
        *,
        bytes: int,
        threads: int = 2,
    ) -> None:
        native = getattr(_ext, "_VarenaLoadSlot", None)
        if native is None:
            raise RuntimeError("this TensorFS build has no native load/execution slot")
        self._native: _ext._VarenaLoadSlot = native(
            arena._native, checkpoint, list(parts), bytes, threads
        )
        self._tensor: Tensor | None = None
        self._decoded: tuple[str, tuple[int, ...]] | None = None
        self._tensors: tuple[Tensor, ...] | None = None
        self._existing_regions = False
        self.device = arena.device

    @classmethod
    def encoded(
        cls,
        arena: Arena,
        checkpoint: _ext.Checkpoint,
        component: str,
        key: str,
        *,
        dtype: str,
        max_scratch_bytes: int,
        threads: int = 2,
    ) -> LoadSlot:
        """One encoded tensor read/decode followed by native execution.

        dtype is F16, BF16 or F32. The required scratch bound includes aligned packed
        parts and the decoder's private GPU allocation sizes; it excludes final output,
        shared read staging and CUDA driver/module/allocation-granularity overhead.
        Metadata, catalog and budget checks precede source I/O. Checked ordering validates
        decoded input before submitting compute. Event ordering guards the complete program
        on the device instead; a late device/ownership failure does not imply zero execution.
        The initialized typed tensor is capture-only until submission completes, using the
        same owning alias and sealing as raw slots.
        """
        native = getattr(_ext, "_VarenaLoadSlot", None)
        if native is None or not hasattr(native, "encoded"):
            raise RuntimeError("this TensorFS build has no encoded native load slot")
        result = cls.__new__(cls)
        result._native = native.encoded(
            arena._native, checkpoint, component, key, dtype=dtype,
            max_scratch_bytes=max_scratch_bytes, threads=threads,
        )
        result._tensor = None
        result._tensors = None
        result._existing_regions = False
        result._decoded = (dtype, tuple(checkpoint.components[component].tensors[key].shape))
        result.device = arena.device
        return result

    @classmethod
    def from_regions(
        cls,
        checkpoint: _ext.Checkpoint,
        groups: Sequence[tuple[Region, Sequence[tuple[str, int, int]]]],
        *,
        threads: int = 2,
        keep_ready: bool = False,
    ) -> LoadSlot | None:
        """Capture against existing immutable raw block regions, then load misses once.

        Native preparation pins every ready input before admitting cold fills. Only missing
        inputs are initialized for capture and later read; their regions remain independently
        cached after completion. None means all inputs were ready at this observation; the
        caller must reacquire and revalidate its ordinary resident binding before capture.
        keep_ready=True instead retains the ready inputs as a capture slot, so an own-address
        native stage can detach its temporary views and refill after later eviction.
        Each region must already belong exclusively to the declared immutable source recipe.
        Authored computation must treat every input as read-only: successful completion
        caches its bytes under that source identity. Runtime rejects mutable-state overlap
        and visible ATen writes; custom/native kernels must honor the same read-only contract.
        """
        native = getattr(_ext, "_VarenaLoadSlot", None)
        if native is None or not hasattr(native, "from_regions"):
            raise RuntimeError("this TensorFS build has no region loading capability")
        if keep_ready and not cls.supports_ready_stage:
            raise RuntimeError("this TensorFS build cannot detach ready stage inputs")
        options = {"keep_ready": True} if keep_ready else {}
        owned = native.from_regions(
            checkpoint, [(region._native, list(parts)) for region, parts in groups], threads, **options
        )
        if owned is None:
            return None
        result = cls.__new__(cls)
        result._native = owned
        result._tensor = None
        result._decoded = None
        result._tensors = None
        result._existing_regions = True
        result.device = owned.device
        return result

    @property
    def immutable_inputs(self) -> tuple[Tensor, ...]:
        """Existing-region source bytes that authored computation must never write."""
        return self.tensors if self._existing_regions else ()

    @property
    def tensors(self) -> tuple[Tensor, ...]:
        """Capture-only input views, in the declared region order."""
        if self._tensors is None:
            import torch

            self._tensors = (
                (self.tensor,) if self._decoded is not None
                else tuple(torch.from_dlpack(pin) for pin in self._native.capture_inputs())
            )
        return self._tensors

    @property
    def tensor(self) -> Tensor:
        """Initialized capture storage (U8 for raw slots, typed/shaped for encoded slots)."""
        if self._tensor is None:
            import torch

            self._tensor = torch.from_dlpack(self._native)
            if self._decoded is not None:
                dtype, shape = self._decoded
                self._tensor = self._tensor.view(
                    {"F16": torch.float16, "BF16": torch.bfloat16, "F32": torch.float32}[dtype]
                ).reshape(shape)
        return self._tensor

    def seal(self) -> None:
        """Called after successful known-body capture; no new capture views are exported."""
        self._native.seal()

    def submit_native(
        self, continuation: object, *, event_ordered: bool = False
    ) -> _ext._VarenaLoadJob:
        """Import the private Runtime capability and start its owning native read worker."""
        if event_ordered:
            if not self.supports_event_input:
                raise RuntimeError("this TensorFS build has no event-ordered native load")
            return self._native.submit_native(continuation, event_ordered=True)
        # Old native extensions accept only the original positional continuation.
        return self._native.submit_native(continuation)

    def cancel(self) -> None:
        """Cancel before submission or request native stop; the job's wait proves completion."""
        self._native.cancel()

    def close(self) -> None:
        """Retire an unused slot or request cancellation; existing tensor aliases stay owned."""
        self._native.close()
        self._tensor = None
        self._tensors = None


def supports_load_execution() -> bool:
    """Whether this build accepts the private continuation v1 capability."""
    return hasattr(_ext, "_VarenaLoadSlot") and hasattr(_ext, "_VarenaLoadJob")
