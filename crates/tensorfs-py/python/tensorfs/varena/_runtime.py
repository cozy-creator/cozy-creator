"""Private TensorFS/Runtime source, graph and owner integration.

This is not a second public allocator API. Runtime and native qualification clients
use these operations to retain their existing source and graph ownership contracts.
Applications use the canonical tensorfs.varena Gpu interface.
"""

from __future__ import annotations

import operator
import weakref
from collections.abc import Callable, Iterator, Sequence
from contextlib import contextmanager
from typing import TYPE_CHECKING

from .. import _ext
from ._facts import (
    CacheOwnerFacts,
    CacheStats,
    DeviceStats,
    HostAddressing,
    HostCacheStats,
    PagesStats,
    PhaseControllerFacts,
    PlanFacts,
)
from .sources import dtype_descriptor

if TYPE_CHECKING:
    import torch
    from torch import Tensor
    from typing_extensions import Buffer, CapsuleType

    from .._ext import RowRead


def activate(device: int, *, shared: bool) -> None:
    """This GPU's context and this process's row in its ledger. `shared` is the machine's mode: it
    yields idle memory to foreign programs; every participant donates to every other either way.
    """
    _ext._varena_activate(device, shared)


def host_room() -> int:
    """What more this process may hold in host RAM: the room the machine's one host admission asks
    against (the tightest limit on this process's path less every executor's holdings and the
    headroom). Negative: over it. Unbounded (2**63 - 1) where the figures are unreadable, as the
    admission then refuses nothing. A CPU executor's weights draw on it."""
    return _ext._varena_host_room()


def poisoned() -> str | None:
    """Why native state can no longer be trusted, if it can't. Opens nothing."""
    return _ext._varena_poisoned()


def request_boundary(device: int, *, begin: bool) -> None:
    """A request started or ended here: orders who donates first, and the first to begin is a phase
    boundary. Never fails."""
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

    def facts(self) -> CacheOwnerFacts:
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

    def region(self, bytes: int, source: Source, set: str | None = None) -> Region:
        """A Region of `bytes` from `source`: address space only, until it is used. `set`: the
        model component it belongs to; its small regions share pages (cr-317)."""
        return Region(self._native.region(bytes, source._native, set))

    def cache(self, reserve: int, shrink: Callable[[int], bool] | None = None) -> Cache:
        """A Cache of `reserve` bytes of this GPU's address space, nothing mapped (`Cache`)."""
        return Cache(self, reserve, shrink)

    def stats(self) -> DeviceStats:
        return self._native.stats()


#: every Cache not yet closed (`cache_ranges`)
_CACHES: weakref.WeakSet[Cache] = weakref.WeakSet()


class Cache:
    """A reserved range of a GPU's address space whose owner maps and unmaps exact pages: where an
    engine keeps what it writes and reads every step (K/V, state). `base` never changes, so a CUDA
    graph may hold addresses inside it. The core never picks a page to drop: short of memory it
    asks `shrink(requested_bytes)`, on its own worker thread with the GIL, which releases what it
    chooses (claiming its own locks without blocking) and returns False to decline. Only completed
    releases count as memory. The Cache keeps `shrink`; the core holds it weakly."""

    def __init__(
        self, device: Device, reserve: int, shrink: Callable[[int], bool] | None = None
    ) -> None:
        self._shrink = shrink
        self._native = _ext._VarenaCache(device._native, reserve, shrink)
        self.device = device.device
        _CACHES.add(self)

    @property
    def base(self) -> int:
        return self._native.base

    @property
    def granularity(self) -> int:
        """The page size: every `commit` and `release` is whole pages at a page offset."""
        return self._native.granularity

    @property
    def reserved_bytes(self) -> int:
        return self._native.reserved_bytes

    def commit(self, offset: int, bytes: int) -> None:
        """Map `bytes` at `offset`, none of it mapped before: all or none. Short of room the core
        reclaims in its one order first; a refusal (torch's out-of-memory type) leaves the pages
        mapped before it as they were."""
        self._native.commit(offset, bytes)

    def release(self, offset: int, bytes: int, stream: int) -> None:
        """Give `bytes` at `offset` back, every reader and writer of them joined into `stream`
        (`torch.cuda.Stream.cuda_stream`). They are memory again once `stream` passes this point."""
        self._native.release(offset, bytes, stream)

    def tensor(
        self, dtype: str | torch.dtype, shape: Sequence[int], byte_offset: int = 0
    ) -> Tensor:
        """A tensor of `shape` at `byte_offset`, anywhere in the reservation. It owns no page: read
        and write only what is committed (a kernel that touches a page that isn't faults the
        device). The Cache cannot close while one is alive."""
        from torch.utils.dlpack import from_dlpack

        code, bits, _ = dtype_descriptor(dtype)
        dims = [operator.index(value) for value in shape]
        view: Tensor = from_dlpack(self._native.dlpack(code, bits, dims, operator.index(byte_offset)))
        return view

    def stats(self) -> CacheStats:
        """Its bytes by state, completed releases collected first."""
        return self._native.stats()

    def close(self) -> None:
        """Return the address space. Refused, changing nothing, while a page is live or its release
        hasn't passed, while a tensor of it is alive, and from its own shrink callback."""
        self._native.close()
        _CACHES.discard(self)


def cache_ranges(device: int) -> list[tuple[int, int, Cache]]:
    """Each open Cache's reservation on `device` as (first address, end, Cache), by address: what
    an observer of device writes classes as an owner's state, not working memory."""
    rows = [
        (cache.base, cache.base + cache.reserved_bytes, cache)
        for cache in list(_CACHES)
        if cache.device == device
    ]
    return sorted(rows, key=lambda row: row[0])


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

    def facts(self) -> PlanFacts:
        return self._native.facts()


class PhaseController:
    """Internal per-submission phase decoration using native continuation ownership."""

    def __init__(self, device: Device) -> None:
        self._native = _ext._VarenaPhaseController(device._native)

    def wrap(
        self, capsule: CapsuleType, key: str | None, regions: Sequence[Region], *, execution_key: str | None = None
    ) -> CapsuleType:
        """Decorate native completion; key=None observes without entering a phase."""
        return self._native.wrap(capsule, key, [region._native for region in regions], execution_key=execution_key)

    def facts(self) -> PhaseControllerFacts:
        return self._native.facts()


class Pages:
    """One GPU's host tier: weight pages in pinned host RAM, which TensorFS alone manages."""

    def __init__(
        self, device: int, *, cap: int | None = None, shared: bool = False, headroom: int | None = None
    ) -> None:
        """The tier sizes itself from the host and shares it through the host's ledger page.
        `cap`: at most this many bytes (default half of physical memory). `shared`: a person's
        computer, where nothing pins while it stalls. `headroom` is an older Runtime's, unread."""
        del headroom
        self._native = _ext._VarenaPages(device, cap, shared)

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

    def read_ahead(self, sources: Sequence[Source]) -> None:
        """Read `sources`' pages ahead of their use, in this order, in place of what was queued:
        a slot's derived page on disk, else a derive's stored bytes, else a plain page's stored
        bytes, each into pinned host RAM within free room. The first that does not fit ends it."""
        self._native.read_ahead([source._native for source in sources])

    def retained_room(self) -> int:
        """Host bytes retained regions may take now: available beyond the headroom, this tier's
        own pages included, which yield to them."""
        return self._native.retained_room()

    def staging(self, bytes: int) -> Staging | None:
        """`bytes` of shared host memory the executors of a group exchange through, pinned for
        this GPU and held against varena's host number until it closes; None: no room now."""
        native = self._native.staging(bytes)
        return None if native is None else Staging(native)

    def cache(self, reserve: int) -> HostCache:
        """A HostCache of `reserve` bytes of host address space for this GPU, nothing mapped.
        Raises, in one line, where `host_addressing` says the device takes no mapped registration."""
        return HostCache(self, reserve)

    def drain(self) -> list[tuple[str, int, int, str, bool, int, float]]:
        """The fills since the last call: (set, block, bytes, origin, ahead, ns, unix ms)."""
        return self._native.drain()

    def stats(self) -> PagesStats:
        return self._native.stats()


def host_addressing(device: int) -> HostAddressing:
    """What `device` answers about reading registered host memory in place: its attribute
    (`CU_DEVICE_ATTRIBUTE_CAN_USE_HOST_POINTER_FOR_REGISTERED_MEM`), whether a page registered
    with the device-map flag sits at its own address there, the driver's refusal where a call was
    refused; `supported` (a device-mapped registration is taken: a HostCache exists, and a copy
    from it may run inside a graph's conditional body), with `why` when it isn't; and `in_place`
    (a kernel may also read it at its host addresses: `HostCache.tensor`)."""
    return _ext._varena_host_cache_probe(device)


class HostCache:
    """Where an engine keeps the K/V a card can't hold: a reserved range of host address space
    whose owner maps and unmaps whole windows (`granularity`). A mapped window is pinned host
    memory registered for the device, so a copy from it is one transfer, inside a graph's
    conditional body too; where `host_addressing` says `in_place`, a kernel also reads it at the
    same address. Its bytes count against the GPU's host tier while mapped. `base` never changes.
    Nothing is page-locked until a window is committed."""

    def __init__(self, pages: Pages, reserve: int) -> None:
        self._native = _ext._VarenaHostCache(pages._native, reserve)

    @property
    def base(self) -> int:
        return self._native.base

    @property
    def granularity(self) -> int:
        """The window size: every `commit` and `release` is whole windows at a window offset."""
        return self._native.granularity

    @property
    def reserved_bytes(self) -> int:
        return self._native.reserved_bytes

    def commit(self, offset: int, bytes: int) -> bool:
        """Map `bytes` at `offset`, none of it mapped before: all or none. False: the host has no
        room for them now (the tier's rule), and nothing was mapped."""
        return self._native.commit(offset, bytes)

    def release(self, offset: int, bytes: int) -> None:
        """Give `bytes` at `offset` back: mapped windows nothing queued after this call reads or
        writes. It waits for the device's queued work."""
        self._native.release(offset, bytes)

    def tensor(self, dtype: str | torch.dtype, shape: Sequence[int], byte_offset: int = 0) -> Tensor:
        """A CUDA tensor of `shape` at `byte_offset`, anywhere in the reservation, as the device
        addresses it: a kernel given it reads host memory in place. Touch only mapped windows."""
        return self._view(dtype, shape, byte_offset, host=False)

    def host(self, dtype: str | torch.dtype, shape: Sequence[int], byte_offset: int = 0) -> Tensor:
        """The same bytes as a host tensor: what a copy to the card reads from."""
        return self._view(dtype, shape, byte_offset, host=True)

    def _view(
        self, dtype: str | torch.dtype, shape: Sequence[int], byte_offset: int, *, host: bool
    ) -> Tensor:
        from torch.utils.dlpack import from_dlpack

        code, bits, _ = dtype_descriptor(dtype)
        dims = [operator.index(value) for value in shape]
        capsule = self._native.dlpack(code, bits, dims, operator.index(byte_offset), host)
        view: Tensor = from_dlpack(capsule)
        return view

    def stats(self) -> HostCacheStats:
        return self._native.stats()

    def close(self) -> None:
        """Refused while a window is mapped or a tensor of it is alive."""
        self._native.close()


class Staging:
    """Host bytes the executors of a group exchange through, mapped and pinned for this GPU at
    `ptr`. The owner's (`Pages.staging`) counts against varena's host number; a peer's mapping of
    it (`Staging.open`, by `name`) counts nothing. Close it before its last copy's memory goes."""

    def __init__(self, native: _ext._VarenaStaging) -> None:
        self._native = native

    @classmethod
    def open(cls, device: int, name: str, bytes: int) -> Staging:
        return cls(_ext._VarenaStaging.open(device, name, bytes))

    @property
    def name(self) -> str:
        return self._native.name

    @property
    def ptr(self) -> int:
        return self._native.ptr

    @property
    def bytes(self) -> int:
        return self._native.bytes

    def unlink(self) -> None:
        """Its name goes, once every peer has opened it: nothing outlives the processes."""
        self._native.unlink()

    def close(self) -> None:
        self._native.close()


class Source:
    """Where a Region's bytes come from: its page in `pages`, else `parts` of `checkpoint`, then
    `derive`, on the thread that called `use`, never during capture. `derive(view)` gets the Region
    as a U8 Tensor, stored bytes in it and gaps zeroed, and returns whether they are now different.

    parts: (component/key#role, destination offset, stored length). `page` names it in the tier;
    None: never kept. `set` and `block` name its fills in the record. `slot` (a sha256 digest, with
    `page` one too) names its block under its fill identity whatever the overlay: a derive's kept
    page is written to the store under it, and a later process reads it from disk instead of the
    store and its derive.
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
        slot: str | None = None,
    ) -> None:
        self._native = _ext._VarenaSource(
            pages._native, set, block, bytes,
            checkpoint=checkpoint, parts=list(parts), page=page, derive=derive, slot=slot,
        )

    def read_host(self, out: Buffer, *, stored: bool = False) -> None:
        """Its bytes in a writable host buffer: its kept page unless `stored`, else the store."""
        self._native.read_host(out, stored=stored)

    def read_rows(self, reads: Sequence[RowRead], *, stream: int | None = None) -> tuple[int, int]:
        """`Checkpoint.read_rows` over the tensors it places: by DMA from its kept page where that
        page holds its stored bytes (a CUDA out on the tier's device), else from the store. Its
        page is never made here (`Pages.read_ahead` keeps one). Returns the bytes served from its
        page and from the store."""
        return self._native.read_rows(reads, stream=stream)


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

    def use_ahead(self, stream: int) -> Use | None:
        """`use` ahead of demand, by the rule the core's own fill worker follows: pinned for `stream`
        once its Source ran here (a derive, which the worker never runs), else None where the core
        declines (no room by that rule, already present or filling, a capture running). cr-330."""
        native = self._native.use_ahead(stream)
        return None if native is None else Use(native)

    def pin(self) -> Use | None:
        """Pin existing ready contents atomically, or return None for a residency miss.

        Never allocates, maps, refills or starts recovery. Errors such as poison and
        use after fork propagate.
        """
        native = self._native.pin()
        return None if native is None else Use(native)


def fill(regions: Sequence[Region], sources: Sequence[Source], stream: int) -> list[Use | None]:
    """Each Region, bound to the matching Source, pinned for `stream`, the absent ones filled
    together: all one store read covers in that one read, pieces pipelined across Regions; any
    other by its own Source. None for a Region not filled here: its `use` fills it, waits for the
    fill already running, or raises why it can't (cr-318: a binding's thousands of small blocks)."""
    natives = _ext._varena_fill(
        [region._native for region in regions], [source._native for source in sources], stream
    )
    return [None if native is None else Use(native) for native in natives]


class Retained:
    """State a model writes on the card once and reads every step (a prefix's K/V): a Region
    the core keeps resident while there is room. A pinned host copy, taken behind compute on its
    own stream, refills it once evicted, and the core fills it ahead of its next use."""

    def __init__(self, device: Device, pages: Pages, bytes: int) -> None:
        self._native = _ext._VarenaRetained(device._native, pages._native, bytes)

    def write(self, stream: int, parts: Sequence[tuple[int, int, int]], *, spill: bool = True) -> Use:
        """Copy `parts`, (offset, device address, bytes) of live tensors ordered on `stream`,
        into the region on `stream`, pinned for it. Once, before any other use. `spill`: a host
        copy is taken behind compute and the region is evictable once it lands; else it stays."""
        return Use(self._native.write(stream, list(parts), spill=spill))

    def use(self, stream: int) -> Use:
        """Pinned for `stream`: refilled from its host copy if it was evicted."""
        return Use(self._native.use_on(stream))

    @property
    def landed(self) -> bool:
        """Whether its host copy has landed: from then on the core may evict it."""
        return self._native.landed

    @property
    def resident(self) -> bool:
        return self._native.resident


class Use:
    """Pinned storage. This Use and every independently exported Storage share one checked
    native Pin owner, and Storage references survive this Python object."""

    def __init__(self, native: _ext._VarenaUse) -> None:
        from torch.utils.dlpack import from_dlpack

        self._native = native
        self.tensor = from_dlpack(native)

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

