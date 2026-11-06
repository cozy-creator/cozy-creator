"""Private TensorFS/Runtime source, graph and owner integration.

This is not a second public allocator API. Runtime and native qualification clients
use these operations to retain their existing source and graph ownership contracts.
Applications use the canonical tensorfs.varena Gpu interface.
"""

from __future__ import annotations

from collections.abc import Callable, Iterator, Sequence
from contextlib import contextmanager
from typing import TYPE_CHECKING, Any

from .. import _ext
if TYPE_CHECKING:
    from torch import Tensor
    from typing_extensions import Buffer


def activate(device: int, *, shared: bool) -> None:
    """This GPU's context and this process's row in its ledger. `shared` is the machine's mode: it
    yields idle memory to foreign programs; every participant donates to every other either way.
    """
    _ext._varena_activate(device, shared)


def poisoned() -> str | None:
    """Why native state can no longer be trusted, if it can't. Opens nothing."""
    return _ext._varena_poisoned()


def request_boundary(device: int, *, begin: bool) -> None:
    """A request started or ended here: orders who donates first. An atomic store, never fails."""
    _ext._varena_request_boundary(device, begin)


def bind_torch_allocator() -> None:
    """Private CPU-only binding; Runtime must validate explicit custody before CUDA init."""
    from .torch import _bind_allocator

    _bind_allocator()

CacheOwnerDeferredError = _ext.CacheOwnerDeferredError


class CacheOwner:
    """Internal weak callback integration; the caller retains its shrink callable.

    The callable receives requested bytes as intent, never as a reclaimed-byte credit.
    It runs on the native callback worker with the GIL and capture-only exclusion, and
    must claim owner/domain locks nonblocking before touching their GPU resources.
    Returning False declines this attempt. Returning does not prove physical release.
    """

    @staticmethod
    def capability() -> int:
        return int(_ext._VarenaCacheOwner.CACHE_OWNER_PROTOCOL)

    def __init__(self, device: int, callback: Callable[[int], object]) -> None:
        if self.capability() != 1:
            raise RuntimeError("unsupported native cache-owner protocol")
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


class Device:
    """One GPU's memory authority: this process's every byte on the card, for its lifetime."""

    def __init__(self, device: int) -> None:
        self._native = _ext._VarenaDevice(device)
        self.device = device

    @property
    def granularity(self) -> int:
        return self._native.granularity

    def region(self, bytes: int, source: Source) -> Region:
        """A Region of `bytes` from `source`: address space only, until it is used."""
        return Region(self._native.region(bytes, source._native))

    def stats(self) -> dict[str, int]:
        return self._native.stats()


class Plan:
    """Copied native region intent on one device; no fills, pins or prefetch promise.

    Keep this owner while its phase can be used. deactivate retires active intent
    synchronously; dropping the owner then releases its reservation references safely.
    """

    def __init__(self, device: Device, phases: Sequence[tuple[str, Sequence[Region]]]) -> None:
        self._native = _ext._VarenaPlan(
            device._native, [(name, [region._native for region in regions]) for name, regions in phases],
        )

    def activate(self, phase: str) -> None:
        self._native.activate(phase)

    def deactivate(self) -> None:
        self._native.deactivate()

    def facts(self) -> dict[str, Any]:
        return self._native.facts()


class PhaseController:
    """Internal per-submission phase decoration using native continuation ownership."""

    def __init__(self, device: Device) -> None:
        self._native = _ext._VarenaPhaseController(device._native)

    def wrap(self, capsule, key: str | None, regions: Sequence[Region], *, execution_key: str | None = None):
        """Decorate native completion; key=None observes without entering a phase."""
        return self._native.wrap(capsule, key, [region._native for region in regions], execution_key=execution_key)

    def facts(self) -> dict[str, object]:
        return self._native.facts()


class Pages:
    """One GPU's host tier: weight pages in pinned host RAM, which TensorFS alone manages."""

    def __init__(self, device: int, *, cap: int, headroom: int) -> None:
        self._native = _ext._VarenaPages(device, cap, headroom)

    def pending(self) -> bool:
        """Whether pages wait for an idle executor to pin them."""
        return self._native.pending()

    def pin_idle(self, waiting: Callable[[], bool]) -> None:
        """Pin them until `waiting()`: it is asked between windows of a page's read."""
        self._native.pin_idle(waiting)

    def forget(self, set: str | None = None) -> None:
        """`set`'s pages go, or every page."""
        self._native.forget(set)

    def request_ended(self) -> None:
        self._native.request_ended()

    def drain(self) -> list[tuple[str, int, int, str, bool, int, float]]:
        """The fills since the last call: (set, block, bytes, origin, ahead, ns, unix ms)."""
        return self._native.drain()

    def stats(self) -> dict[str, Any]:
        return self._native.stats()


class Source:
    """Where a Region's bytes come from: its page in `pages`, else `parts` of `checkpoint`, then
    `derive`, on the thread that called `use`, never during capture. `derive(view)` gets the Region
    as a U8 Tensor, stored bytes in it and gaps zeroed, and returns whether they are now different.

    parts: (component/key#role, destination offset, stored length). `page` names it in the tier;
    None: never kept. `set` and `block` name its fills in the record.
    """

    def __init__(
        self,
        pages: Pages,
        set: str,
        block: int,
        bytes: int,
        *,
        checkpoint: _ext.Checkpoint | None = None,
        parts: Sequence[tuple[str, int, int]] = (),
        page: str | None = None,
        derive: Callable[[Tensor], object] | None = None,
    ) -> None:
        self._native = _ext._VarenaSource(
            pages._native, set, block, bytes,
            checkpoint=checkpoint, parts=list(parts), page=page, derive=derive,
        )

    def read_host(self, out: Buffer, *, stored: bool = False) -> None:
        """Its bytes in a writable host buffer: its kept page unless `stored`, else the store."""
        self._native.read_host(out, stored=stored)


class Region:
    def __init__(self, native: _ext._VarenaRegion) -> None:
        self._native = native

    @property
    def resident(self) -> bool:
        return self._native.resident

    @property
    def allocated_bytes(self) -> int:
        return self._native.allocated_bytes

    @property
    def ptr(self) -> int:
        return self._native.ptr

    def use(self, stream: int) -> Use:
        """Pinned for `stream`, a live ``torch.cuda.Stream.cuda_stream``. Absent, its Source fills
        it; the core makes room in its one reclaim order and alone chooses what else moves."""
        return Use(self._native.use_on(stream))

    def pin(self) -> Use | None:
        """Pin existing ready contents atomically, or return None for a residency miss.

        Never allocates, maps, refills or starts recovery. Errors such as poison and
        use after fork propagate.
        """
        native = self._native.pin()
        return None if native is None else Use(native)


class Use:
    """Pinned storage. This Use and every independently exported Storage share one checked
    native Pin owner, and Storage references survive this Python object."""

    def __init__(self, native: _ext._VarenaUse) -> None:
        import torch

        self._native = native
        self.tensor = torch.from_dlpack(native)

    def consume_on(self, stream: int) -> None:
        """Declare a CUDA stream (``torch.cuda.Stream.cuda_stream``) that uses this storage.

        The final shared owner records events on live declared streams, or uses their
        checked retirement witnesses. Declare every consumer, including escaped aliases,
        before submission; finish declarations before retiring/destroying a stream.
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

