"""CUDA weight regions and shared process memory constraints.

Install tensorfs.varena.torch for Torch working allocations; native TensorFS device buffers
always use the same Varena working allocator.
"""

from __future__ import annotations

from collections.abc import Callable, Iterator, Sequence
from contextlib import contextmanager
from typing import TYPE_CHECKING, Any

from .. import _ext
from .api import Gpu as Gpu, open as open
from .cooperative import Cooperation as Cooperation
from .cooperative import CooperationDeferredError as CooperationDeferredError
from .cooperative import CooperationOwner as CooperationOwner
from .cooperative import CooperationStaleSampleError as CooperationStaleSampleError

if TYPE_CHECKING:
    from torch import Tensor
    from typing_extensions import Buffer

try:
    PoisonedError = _ext.VarenaPoisonedError
except AttributeError as error:
    raise RuntimeError(
        "TensorFS native Varena capability is missing: VarenaPoisonedError"
    ) from error

def make_room(device: int, limit: int | None) -> int:
    """Hold this process's GPU bytes on `device` (resident weight pages plus mapped working allocations) to
    `limit`, None for no limit; returns the pages taken back now. Fills and working allocations stay
    inside it until it changes: what the machine asks when another program needs the GPU."""
    return _ext._varena_make_room(device, limit)


class Cache:
    """Owner-addressed CUDA pages; raw addresses are not tensor or graph lifetime leases.

    Retain this owner while any consumer may use its reservation. Before release, stop
    new use of the exact range and join every consumer into the supplied stream. Poll
    collects completed releases without waiting. No shrink callback is installed here.
    """

    def __init__(self, device: int, reserve_bytes: int) -> None:
        self._native = _ext._VarenaCache(device, reserve_bytes)
        self.device = device

    @property
    def base(self) -> int:
        return self._native.base

    @property
    def granularity(self) -> int:
        return self._native.granularity

    @property
    def reserved_bytes(self) -> int:
        return self._native.reserved_bytes

    def commit(self, offset: int, bytes: int) -> None:
        """Map an entirely free, page-aligned range; capacity failure rolls it back."""
        self._native.commit(offset, bytes)

    def release(self, offset: int, bytes: int, stream: int = 0) -> None:
        """Release only the owner's joined pages after this CUstream completes."""
        self._native.release(offset, bytes, stream)

    def poll(self) -> dict[str, int]:
        """Collect completed releases without waiting on future work."""
        return self._native.poll()

    def stats(self) -> dict[str, int]:
        return self._native.stats()

    def close(self) -> None:
        """Refuse live/pending pages; return the reservation after release completes."""
        self._native.close()


class _CacheOwnerDeferredError(RuntimeError):
    pass


CacheOwnerDeferredError = getattr(_ext, "CacheOwnerDeferredError", _CacheOwnerDeferredError)


class CacheOwner:
    """Internal weak callback integration; the caller retains its shrink callable.

    The callable receives requested bytes as intent, never as a reclaimed-byte credit.
    It runs on the native callback worker with the GIL and capture-only exclusion, and
    must claim owner/domain locks nonblocking before touching their GPU resources.
    Returning False declines this attempt. Returning does not prove physical release.
    """

    @staticmethod
    def capability() -> int:
        return int(getattr(getattr(_ext, "_VarenaCacheOwner", None), "CACHE_OWNER_PROTOCOL", 0))

    def __init__(self, device: int, callback: Callable[[int], object]) -> None:
        if self.capability() != 1:
            raise RuntimeError("this TensorFS has no native cache-owner capability")
        self._native = _ext._VarenaCacheOwner(device, callback)

    def touch(self) -> None:
        self._native.touch()

    def check(self) -> None:
        self._native.check()

    def suspend(self) -> None:
        self._native.suspend()

    def facts(self) -> dict[str, object]:
        return self._native.facts()

    def defer_close(self) -> None:
        """Disable new requests without waiting for a callback or its runtime."""
        self._native.defer_close()

    def close(self) -> None:
        """Drain outside caller owner locks; capture/current-worker refusal retains data."""
        self._native.close()


def request_cache_shrink(device: int, bytes: int) -> _ext._VarenaCacheRequest | None:
    """Queue local owner intent without waiting or treating the request as free memory."""
    return _ext._varena_cache_request(device, bytes)


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

    def clear_streaming_limit(self) -> None:
        """Clear the legacy idle-cache window without reclaim or changing capacity limits."""
        self._native.clear_streaming_limit()

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


def supports_phase_plans() -> bool:
    """Whether copied native phase-region intent is available in this extension."""
    return hasattr(_ext, "_VarenaPlan")


class Plan:
    """Copied native region intent for one arena; no fills, pins or prefetch promise.

    Keep this owner while its phase can be used. deactivate retires active intent
    synchronously; dropping the owner then releases its reservation references safely.
    """

    def __init__(self, arena: Arena, phases: Sequence[tuple[str, Sequence[Region]]]) -> None:
        self._native = _ext._VarenaPlan(
            arena._native, [(name, [region._native for region in regions]) for name, regions in phases],
        )

    def activate(self, phase: str) -> None:
        self._native.activate(phase)

    def deactivate(self) -> None:
        self._native.deactivate()

    def facts(self) -> dict[str, Any]:
        return self._native.facts()


class PhaseController:
    """Internal per-submission phase decoration using native continuation ownership."""

    def __init__(self, arena: Arena) -> None:
        self._native = _ext._VarenaPhaseController(arena._native)

    def wrap(self, capsule, key: str | None, regions: Sequence[Region], *,
             footprints: Sequence[_ext._VarenaGraphFootprint] | None = None, execution_key: str | None = None):
        """Decorate native completion; key=None observes without changing weight preferences."""
        return self._native.wrap(capsule, key, [region._native for region in regions],
                                 footprints=None if footprints is None else list(footprints), execution_key=execution_key)

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

    def prepare(self, *, reclaim: bool = True, evict_completed: bool = False) -> Use:
        """A fill, or the resident contents. reclaim=False refuses (OutOfMemory) rather than
        evict or wait; evict_completed then still evicts idle streamed regions (other
        phases' oldest first, then the running phase's most recently used first), completed
        ones first, then ones after their own last-consumer events; it never joins."""
        return Use(self._native.prepare(reclaim, evict_completed))

    def pin(self) -> Use | None:
        """Pin existing ready contents atomically, or return None for a residency miss.

        Never allocates, maps, refills or starts recovery. Errors such as poison and
        use after fork propagate. The returned Use and its aliases own the same physical
        pin as prepare(); this adds no ordering for writes after the committed fill.
        """
        native = self._native.pin()
        return None if native is None else Use(native)

    def prepare_prefetch(
        self,
        checkpoint: _ext.Checkpoint,
        parts: Sequence[tuple[str, int, int]],
        *,
        threads: int = 2,
    ) -> PreparedRead | None:
        """Opt into native no-reclaim admission for an immutable raw source.

        None means no fill was admitted (unknown/unqualified key, insufficient room, or
        already resident). An admitted read retains its source and Fill but starts no IO.
        Existing production prefetch callers remain on prepare_read until a committed
        execution key is available. Encoded decode/scratch is outside this first slice.
        """
        native = _ext._VarenaPreparedRead.prefetch(self._native, checkpoint, list(parts), threads)
        return None if native is None else PreparedRead(native)

    def prepare_read(
        self,
        checkpoint: _ext.Checkpoint,
        parts: Sequence[tuple[str, int, int]],
        *,
        threads: int = 2,
        reclaim: bool = True,
        evict_completed: bool = False,
        host: Buffer | None = None,
    ) -> PreparedRead:
        """Admit a fill and warm bounded staging, without starting source I/O.

        Each part is (component/key#role, destination byte offset, exact stored length).
        This region must be dedicated to that immutable source recipe. Native prefetch
        uses local available sources only; it never calls a Python transformer/fetcher.
        reclaim=False refuses admission when eviction or a prior-writer join is needed;
        with evict_completed it may still evict idle streamed regions as `prepare` does.
        host, a writable C-contiguous buffer covering every part and at most the region's
        length, receives the one store read; one copy then fills the region from it (zeroes
        past it). The read row's
        host_bytes says whether it was filled (0 when the region was already resident).
        """
        native = getattr(_ext, "_VarenaPreparedRead", None)
        if native is None:
            raise RuntimeError(
                "this TensorFS build has no native owned refill capability"
            )
        return PreparedRead(
            native(
                self._native, checkpoint, list(parts), threads, reclaim, evict_completed, host
            )
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


def supports_host_prefetch() -> bool:
    """Whether prepare_read accepts host=: one store read into a caller's host page."""
    return bool(getattr(getattr(_ext, "_VarenaPreparedRead", None), "HOST", False))


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


@contextmanager
def no_capture(device: int) -> Iterator[None]:
    """No CUDA capture is in progress on `device`, and none begins until it exits (one that
    tries is refused). Raises if one is in progress."""
    guard = _ext._VarenaNoCapture(device)
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
