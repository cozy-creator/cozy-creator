"""Whole-process Torch working allocator. Install before CUDA initialization in the executor child.

Capture with this module's graph/replay owners. A raw torch.cuda.graph supplies none of the
lifetime signals the allocator needs, so its capture is refused.
"""
from __future__ import annotations

import ctypes
from contextlib import contextmanager
import functools
import os
from pathlib import Path
import platform
import threading
from typing import TYPE_CHECKING, Any, Iterator, Literal, Sequence, TypeAlias, cast

from .. import _ext

PhaseOverlapError = _ext.PhaseOverlapError

if TYPE_CHECKING:
    from torch import device as TorchDevice
    from torch.cuda import Stream

DeviceLike: TypeAlias = "int | str | TorchDevice | None"
_ExplicitDevice: TypeAlias = "int | str | TorchDevice"
PoolId: TypeAlias = tuple[int, int]

_lock = threading.Lock()
_owner = 0
_library: ctypes.CDLL | None = None
_workspaces_known: set[int] = set()
_CAPTURE_CAPACITY_PREFIX = "[tensorfs.varena.capture_capacity.v1] "


class _Failure(ctypes.Structure):
    _fields_ = [
        ("kind", ctypes.c_int32),
        ("cuda", ctypes.c_int32),
        ("message", ctypes.c_char * 512),
    ]


@functools.cache
def _exhausted() -> type:
    import torch

    from .api import Exhausted

    class Refused(Exhausted, torch.OutOfMemoryError):
        """varena's `Exhausted` from a Torch-side call (`split_select`, `split_begin`,
        `prepare_pool`, a region's use): nothing more could be reclaimed for it, and the work it was
        asked for has not run. Catch it as `tensorfs.varena.Exhausted`; it is still torch's OOM."""

        def __init__(self, message: str, requested_bytes: int = 0, available_bytes: int = 0) -> None:
            super().__init__(message, requested_bytes)
            self.available_bytes = available_bytes

    Refused.__name__ = Refused.__qualname__ = "Exhausted"
    return Refused


def _refused(message: str, requested_bytes: int, available_bytes: int) -> BaseException:
    """The exception `_ext` raises for varena's `Exhausted` and `OutOfMemory`."""
    return cast(BaseException, _exhausted()(message, requested_bytes, available_bytes))


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


def install(device: _ExplicitDevice = 0) -> None:
    """Select Varena and activate a device, defaulting to device0, outside capture.

    The wheel must carry a shim built for this exact Torch (`torch.__version__`).
    This never compiles at runtime and uses no cudaMalloc import hook.
    """
    index = _activation_device(device)
    _bind_allocator()
    initialize(index)


def _activation_device(device: DeviceLike) -> int:
    """Resolve a requested ordinal without Torch's lazy current-device initialization."""
    if device is None:
        raise TypeError("activation requires an explicit device; None is not a CPU-only mode")
    if isinstance(device, int):
        index = device
    else:
        import torch

        parsed = torch.device(device)
        if parsed.type != "cuda":
            raise ValueError("Varena requires a CUDA device")
        index = 0 if parsed.index is None else parsed.index
    if index < 0:
        raise ValueError("device must be nonnegative")
    return index


def _bind_allocator() -> None:
    """Private Runtime CPU-only selection; device activation requires its explicit Start."""
    global _owner, _library
    import torch

    if _owner and _owner != os.getpid():
        _installed()  # Reject before touching a potentially inherited installation lock.
    with _lock:
        if _owner:
            _installed()
        else:
            if torch.cuda.is_initialized():
                raise RuntimeError("install Varena before Torch CUDA lazy initialization")
            # cuBLAS workspaces cached per handle and stream, as varena accounts them
            # (`initialize`): Torch latches this at its first cuBLAS use.
            os.environ["TORCH_CUBLAS_WORKSPACE_CACHE"] = "1"
            if torch.backends.cudnn.benchmark:
                raise RuntimeError("Varena requires cudnn.benchmark=False")
            directory = Path(__file__).resolve().parents[1]
            runtime_torch = str(torch.__version__)
            built = sorted((p.stem.removeprefix("_varena_torch-") for p in directory.glob("_varena_torch-*.so")),
                           key=lambda v: [int(n) for n in v.split("+")[0].split(".")])
            if not built:
                raise RuntimeError(f"this TensorFS wheel has no Torch allocator shim for {platform.system()} {platform.machine()}: GPU work runs on Linux x86_64 or aarch64, so rent one with `cozy machine rent`")
            if runtime_torch not in built:
                floor = ".".join(built[0].split(".")[:2])
                raise RuntimeError(f"this package's environment has Torch {runtime_torch}, GPU work needs Torch {' or '.join(built)}: relax the package's torch pin to torch>={floor}, or update the machine to a TensorFS built for Torch {runtime_torch}")
            native = ctypes.CDLL(str(directory / f"_varena_torch-{runtime_torch}.so"))
            native.varena_torch_bind.argtypes = [ctypes.c_void_p]
            native.varena_torch_bind.restype = ctypes.c_int
            native.varena_torch_abort_routing.argtypes = [ctypes.c_int, ctypes.c_uint64, ctypes.c_uint64]
            native.varena_torch_abort_routing.restype = ctypes.c_int
            native.varena_torch_workspace_cache_enabled.argtypes = [
                ctypes.POINTER(ctypes.c_int32), ctypes.POINTER(_Failure)
            ]
            native.varena_torch_workspace_cache_enabled.restype = ctypes.c_int32
            native.varena_torch_storage_owners.argtypes = [ctypes.c_void_p]
            native.varena_torch_storage_owners.restype = ctypes.c_uint64
            bindings = _ext._varena_working_bindings()
            if native.varena_torch_bind(bindings) != 0:
                raise RuntimeError("Varena Torch shim refused its native function table")

            native.varena_torch_install.argtypes = [ctypes.POINTER(_Failure)]
            native.varena_torch_install.restype = ctypes.c_int32
            failure = _Failure()
            if native.varena_torch_install(ctypes.byref(failure)) != 1 or failure.kind:
                raise RuntimeError(bytes(failure.message).decode("utf-8", errors="replace"))
            _library, _owner = native, os.getpid()


def _workspace_cache_enabled() -> bool:
    """Read Torch's actual latched flag without activating a CUDA device."""
    _installed()
    enabled, failure = ctypes.c_int32(), _Failure()
    status = cast(ctypes.CDLL, _library).varena_torch_workspace_cache_enabled(
        ctypes.byref(enabled), ctypes.byref(failure)
    )
    if status != 1 or failure.kind:
        raise RuntimeError(bytes(failure.message).decode("utf-8", errors="replace"))
    return bool(enabled.value)


def initialize(device: _ExplicitDevice = 0) -> None:
    """Activate the verified native service and Working custody outside capture."""
    index = _activation_device(device)
    _installed()
    _ext._varena_working_initialize(index)
    if index not in _workspaces_known:
        _workspaces_known.add(index)
        if _workspace_cache_enabled():
            import torch

            with torch.cuda.device(index):
                sizes = [torch.backends.cuda.cublas_workspace_size(), torch.backends.cuda.cublaslt_workspace_size()]
            _ext._varena_working_library_workspaces(index, sizes)


def stats(device: DeviceLike = 0) -> dict[str, int]:
    _installed()
    return _ext._varena_working_stats(_device(device))


def ooms(device: DeviceLike = 0) -> int:
    """`stats(device)["ooms"]` alone: the allocations varena refused on `device`, one integer read."""
    _installed()
    return _ext._varena_working_ooms(_device(device))


def room(device: DeviceLike = 0) -> dict[str, int]:
    """What the card has now, as a split chooses by: `capacity` (the card or its cap, less what is
    outside this process), `working_live`, `pinned`, `weights_resident` and `free` bytes."""
    _installed()
    return _ext._varena_room(_device(device))


def reset_peak(device: DeviceLike = 0) -> None:
    """Reset measured working peaks to current live and mapped bytes."""
    _installed()
    _ext._varena_working_reset_peak(_device(device))


def site_id(name: str) -> int:
    """The id `set_site` takes for a birth site's name, the same for the process's life."""
    return _ext._varena_site_id(name)


def set_site(device: int, site: int) -> None:
    """Where this device's next working allocations are born (one store, no lock)."""
    _ext._varena_set_site(device, site)


def pass_mark(device: int, head: int, shape: int, floor: int) -> dict[str, Any] | None:
    """A forward of model `head` begins (inputs of `shape`, `floor` its largest block's weight
    bytes). Its first this request begins a loop, later ones are its passes: working blocks an
    earlier pass left live are its escaped bytes. None, or once per loop why its next pass cannot
    fit with every evictable byte gone."""
    return _ext._varena_pass_mark(device, head, shape, floor)


def escapes_take(device: int) -> list[dict[str, Any]]:
    """This request's loops, ended, with what each began with and its passes left live. A refusal
    in the last loop's last pass that varena never answered is learned from: its next loop of
    that shape is refused at its first forward unless the card has more room."""
    return _ext._varena_escapes_take(device)


def checks(*, unmap_idle: bool = False, perturb: bool = False, guards: bool = False) -> None:
    """A gate cell's first call, never a product's. `unmap_idle`: every idle pool's pages go before
    a scratch pool's use. `perturb`: freed and reused blocks and a scratch pool's pages read as a
    nonzero pattern. `guards`: unmapped VA after each large block and pool. Costly while on."""
    _ext._varena_set_checks(unmap_idle | perturb << 1 | guards << 2)


def check_counts() -> dict[str, int]:
    """What `checks` has done in this process, so a cell can show it did something."""
    uses, unmapped, perturbed = _ext._varena_check_counts()
    return {"uses": uses, "unmapped_bytes": unmapped, "perturbed_bytes": perturbed}


def replay_differs(eager: Any, replayed: Any) -> str:
    """Why a replay is not its eager pass bit for bit ("" when it is): every graph's one check
    before it is trusted. Both are pytrees of one shape; NaNs in the same place agree."""
    import torch
    from torch.utils._pytree import tree_leaves

    for index, (a, b) in enumerate(zip(tree_leaves(eager), tree_leaves(replayed), strict=True)):
        if isinstance(a, torch.Tensor):
            count = int(((a != b) & ~(a.isnan() & b.isnan())).sum())
            if count:
                return f"output {index}: {count} of {a.numel()} values differ"
    return ""


def unit_bytes() -> int:
    """How large a block a whole module may be before its children bind instead."""
    return _ext._varena_unit_bytes()


def pool_bytes(peak: int) -> int:
    """A pool's size for a capture from an eager pass's measured peak."""
    return _ext._varena_pool_bytes(peak)


def configure(device: DeviceLike, *, cap: int | None = None, display: int | None = None) -> int:
    """The machine's facts about this card: its development cap and, while it drives a display,
    what the display leaves (None: none). The card reads as the smaller; returns bytes reclaimed."""
    _installed()
    return _ext._varena_configure(_device(device), cap=cap, display=display)


def set_limit(device: DeviceLike, bytes: int | None = None) -> int:
    """Development only: use at most this much of the card, as a smaller card would.

    The core keeps it true against the driver's own charge. Live owners are retained when
    lowering it.
    Returns unused bytes reclaimed now.
    """
    _installed()
    return _ext._varena_working_set_limit(_device(device), bytes)


def split_context(device: int) -> dict[str, Any]:
    """Metadata-only eligibility for an explicit device; no implicit CUDA initialization."""
    _installed()
    if device < 0:
        raise ValueError("device must be nonnegative")
    return _ext._varena_split_context(device)


def split_select(device: DeviceLike, key: str, choices: Sequence[tuple[str, Sequence[int]]], *, stream: int) -> dict[str, Any]:
    """Choose an opaque legal variant from a complete copied allocation menu before execution."""
    _installed()
    return _ext._varena_split_select(_device(device), key, [(name, list(sizes)) for name, sizes in choices], stream=stream)


def split_begin(device: DeviceLike, key: str, choice_id: str, *, stream: int, completion_owner: int | None = None, plan_generation: int | None = None) -> int:
    """Commit exactly this choice and open one nonnested leaf interval within its phase.

    completion_owner is the active eager observation whose checked stream completion also
    covers this leaf. None permits declared selection but cannot train a successful sample.
    Forward the opaque plan_generation returned by split_select. A stale generation refuses
    before execution; omission is supported only before a Plan has participated on this GPU.
    """
    _installed()
    return _ext._varena_split_begin(_device(device), key, choice_id, stream=stream, completion_owner=completion_owner, plan_generation=plan_generation)


@functools.cache
def _final() -> type:
    from .api import Exhausted

    class Final(Exhausted):
        """varena's final answer: no lever left (eviction, spilling, a wait for other programs,
        a finer level, streaming) makes it fit. `refusal` says what it needed beside what this
        process holds, and `next` what to do: "again" (varena learned it; the next call runs
        finer), "wait" (it fits once memory this process holds goes), else "smaller"."""

        def __init__(self, refusal: dict[str, Any]) -> None:
            super().__init__(f"needs {refusal['need_bytes']} bytes, {refusal['room_bytes']} have room", refusal["need_bytes"])
            self.refusal = refusal
            self.next = refusal["next"]

    Final.__name__ = Final.__qualname__ = "Final"
    return Final


def final(refusal: dict[str, Any]) -> BaseException:
    return cast(BaseException, _final()(refusal))


class Leaf:
    """One op at the level varena gave it (`Site.enter`). `token` is its open leaf, None when it
    runs unobserved (small, under a capture or replay, inside another leaf)."""

    __slots__ = ("site", "level", "token", "selection")

    def __init__(self, site: "Site", row: dict[str, Any]) -> None:
        self.site, self.level, self.token, self.selection = site, int(row["level"]), row["token"], row["selection"]

    def exit(self, *, submitted: bool) -> "Leaf | None":
        """Close it. After an allocation varena refused inside it: the Leaf to run again in (Retry:
        a pure site's level, its room made), else `Final`. None: done, or a failure not memory's."""
        if self.token is None:
            return None
        answer = _ext._varena_site_exit(self.site.device, self.token, submitted=submitted)
        self.token = None
        if "retry" in answer:
            return Leaf(self.site, answer["retry"])
        if "refused" in answer:
            raise final(answer["refused"])
        return None


class Site:
    """An op's legal levels on one device, largest first, each its allocations' bytes, registered
    once: varena keeps the useful ones (`levels`), picks the level each op runs at with its room
    made, and answers a refusal inside it. `pure`: the op is the Runtime's own and a function of
    its operands, so it may run again after a refusal; anything else is Final."""

    __slots__ = ("device", "key", "levels", "pure", "_menu")

    def __init__(self, device: int, key: str, menu: Sequence[Sequence[int]], *, pure: bool) -> None:
        _installed()
        self.device, self.key, self.pure = device, key, pure
        self._menu = [list(row) for row in menu]
        self.levels = self._register()

    def _register(self) -> int:
        return cast(int, _ext._varena_site_register(self.device, self.key, self._menu, pure=self.pure))

    def _answer(self, ask: Any) -> Leaf:
        row = ask()
        if "unregistered" in row:  # forgotten with its owners: the same menu again
            self._register()
            row = ask()
        if "refused" in row:
            raise final(row["refused"])
        return Leaf(self, row)

    def enter(self, *, stream: int, owner: int | None = None, level: int | None = None) -> Leaf:
        """The level this op runs at, chosen now (or `level`, agreed or fixed), its leaf open under
        the eager phase `owner`; `Final` when no lever makes any level fit."""
        return self._answer(lambda: _ext._varena_site_enter(
            self.device, self.key, stream=stream, completion_owner=owner, level=level))

    def choose(self, *, stream: int) -> Leaf:
        """The level varena would open now, its room made, and no leaf: what a group agrees on."""
        return self._answer(lambda: _ext._varena_site_choose(self.device, self.key, stream=stream))


def admit(device: DeviceLike, wants: Sequence[tuple[Sequence[Any], tuple[PoolId, int] | None]], *, stream: int) -> tuple[int, int, list[Any]]:
    """Before a phase runs: the first of `wants` varena can hold for `stream`, most first, each the
    Regions it reads and the pool it records into with its measured working peak. Returns the
    index admitted, its pool's bytes and a Use per Region, held while it runs. `Final` when none
    fits with every lever pulled: the caller runs none of them, never a part."""
    _installed()
    rows = [(list(regions), None if pool is None else (cast(PoolId, tuple(pool[0])), int(pool[1]))) for regions, pool in wants]
    answer = _ext._varena_admit(_device(device), rows, stream=stream)
    if "needs" in answer:
        need = min(answer["needs"], default=0)
        raise final(_ext._varena_refusal(_device(device), need) | {"needs": answer["needs"]})
    return int(answer["index"]), int(answer["pool_bytes"]), list(answer["uses"])


def reserve(device: DeviceLike, nbytes: int, *, releasing: int = 0) -> Any:
    """`nbytes` on the current stream as one uint8 tensor, mapped now with every lever varena has:
    the reservation itself, which its caller cuts into its buffers, so nothing can take the room
    between a check and the allocation. `Final` when refused: "wait" when it fits once the
    `releasing` bytes this process holds go, else "smaller"."""
    import torch

    index = _device(device)
    before = ooms(index)
    try:
        return torch.empty(nbytes, dtype=torch.uint8, device=torch.device("cuda", index))
    except torch.OutOfMemoryError as exc:
        if ooms(index) == before:
            raise
        raise final(_ext._varena_refusal(index, nbytes, releasing=releasing)) from exc


@contextmanager
def repeating(device: DeviceLike, repeats: int) -> Iterator[None]:
    """A step loop of `repeats` steps runs inside: every plan phase in it repeats that often, so
    varena keeps its weights and working pages for the next step rather than judging from uses."""
    index = _device(device)
    _ext._varena_repeating(index, repeats)
    try:
        yield
    finally:
        _ext._varena_repeating(index, None)


def split_end(device: DeviceLike, observation_id: int, *, submitted: bool) -> dict[str, Any]:
    """Close host submission; only the enclosing phase's checked completion can publish."""
    _installed()
    return _ext._varena_split_end(_device(device), observation_id, submitted=submitted)


def phase_enter(device: DeviceLike, key: str | None = None, *, mode: Literal["eager", "capture", "replay", "preparation"] = "eager") -> int:
    """Begin one execution observation. An unknown key never enables speculation.

    PhaseOverlapError means another observation already owns this device; entry
    left its token and sample intact. Other admission failures retain their types.
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


def phase_forget(device: DeviceLike, key: str) -> bool:
    """Retire a dead model/plan's measurement after its final execution owner releases it."""
    _installed()
    return _ext._varena_phase_forget(_device(device), key)


def prepare_pool(pool: PoolId, bytes: int, device: DeviceLike = 0) -> int:
    """Begin using this pool: its pages mapped, at least this capacity, at the same addresses
    after any idle release. bytes=0 before a replay and before writing its inputs. A scratch
    pool's contents are undefined between uses: copy every input in again, no check can catch an
    owner that skips it. The previous pool survives failure. record_pool_stream ends the use."""
    initialize(device)
    return _ext._varena_working_pool_reserve(_device(device), cast(PoolId, tuple(pool)), bytes)


def cancel_preparation(pool: PoolId, device: DeviceLike = 0) -> None:
    """Return an unused reservation to reclaimability without releasing a graph owner."""
    _installed()
    _ext._varena_working_pool_cancel_preparation(_device(device), cast(PoolId, tuple(pool)))


def pool_close(pool: PoolId, device: DeviceLike = 0) -> None:
    """The owner reset every graph it captured in this pool and allocates there no more: its
    pages go at the next trim once its blocks are freed, even after a capture whose end failed,
    which Torch never releases. Refused while a capture on it is open."""
    _installed()
    _ext._varena_working_pool_close(_device(device), cast(PoolId, tuple(pool)))


def pool_scratch(pool: PoolId, held: Sequence[Any] = (), device: DeviceLike = 0) -> None:
    """The owner declares the tensors it holds in this pool dead between uses: nothing reads
    them outside prepare_pool(pool, 0) .. record_pool_stream(pool, stream). The core may then
    release the idle pool's pages, keeping its VA and graph. A tensor sharing a held one's
    storage could still reach them, so it refuses this, as does any other live allocation.
    """
    _installed()
    native = cast(ctypes.CDLL, _library)
    storages: dict[int, tuple[Any, int]] = {}
    for tensor in held:
        storage = tensor.untyped_storage()
        storages[storage._cdata] = (storage, storages.get(storage._cdata, (storage, 0))[1] + 1)
    for cdata, (storage, tensors) in storages.items():
        if native.varena_torch_storage_owners(cdata) > tensors + 1:  # its Python wrapper owns one
            raise RuntimeError("a tensor sharing this graph's memory has escaped its owner")
    _ext._varena_working_pool_scratch(_device(device), cast(PoolId, tuple(pool)), [storage.data_ptr() for storage, _ in storages.values()])


def _prime_library(device: int, stream: Stream) -> None:
    """Torch's cached cuBLAS and cuBLASLt workspaces for this thread's handle on this stream, made
    before a capture: any graph's reset drops its stream's, so every capture primes its own.
    Their kernels stay real work, outside any graph."""
    import torch
    import torch.nn.functional as functional

    with torch.cuda.device(device), torch.cuda.stream(stream):
        value = torch.ones((2, 2), dtype=torch.float32, device=device)
        torch.mm(value, value)
        functional.linear(value, value, value[0])


def capture_complete(pool: PoolId, *, stream: Stream, device: DeviceLike = None) -> None:
    """Release capture exclusion after the owner actually ended this stream's capture."""
    _installed()
    import torch

    index = _device(stream.device if device is None else device)
    with torch.cuda.device(index), torch.cuda.stream(stream):
        if torch.cuda.is_current_stream_capturing():
            raise RuntimeError("end CUDA stream capture before completing its Varena pool")
        _ext._varena_working_capture_complete(index, cast(PoolId, tuple(pool)))


def pool_short(pool: PoolId, device: DeviceLike = None) -> int:
    """The largest allocation the pool's last capture had no room for, 0 when none."""
    _installed()
    return _ext._varena_working_pool_short(_device(device), cast(PoolId, tuple(pool)))


def record_pool_stream(pool: PoolId, stream: Stream, device: DeviceLike = None) -> None:
    """After a replay's output use has been submitted on this stream: this use is over."""
    _installed()
    index = _device(stream.device if device is None else device)
    _ext._varena_working_pool_record_stream(index, cast(PoolId, tuple(pool)), stream.cuda_stream, True)


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


def _blocking_stream(device: int) -> Stream:
    """Everything that executes or captures runs on a blocking stream. A library's kernel launched
    with no stream argument keeps order only with one: on a non-blocking stream it races (Xid 31,
    run 28772). A capture on one refuses that launch instead of silently leaving it out of the graph
    (noise after replay, 28892), and the caller falls back to eager. CUDA's runtime makes it, so a
    thread that hasn't touched the device needs no context. It lives until the process exits.
    """
    import torch

    handle = ctypes.c_void_p()
    with torch.cuda.device(device):
        code = int(torch.cuda.cudart().cudaStreamCreate(ctypes.addressof(handle)))
        if code:
            raise RuntimeError(f"cudaStreamCreate failed with CUDA error {code}")
        return torch.cuda.ExternalStream(handle.value, device=device)


@functools.cache
def _graph_stream(device: int) -> Stream:
    return _blocking_stream(device)


@contextmanager
def graph(cuda_graph: Any, *, pool: PoolId | None = None, stream: Stream | None = None, reserve_bytes: int | None = None,
          primed: tuple[Stream, ...] = ()) -> Iterator[Any]:
    """Prepare and finish capture, joining only its explicit stream and the caller. `primed`: other
    streams the captured work runs on; theirs and the capture stream's workspaces are made first,
    so nothing allocated there under capture is classed as one by its size.

    Other producer streams must already be joined by the caller. Native capture
    admission still refuses unrelated producers; pool preparation may allocate/wait.
    Shared-pool replay order remains the owner's contract.
    """
    _installed()
    import torch

    device = _device(stream.device if stream is not None else None)
    caller = torch.cuda.current_stream(device)
    stream = _graph_stream(device) if stream is None else stream
    pool = cast(PoolId, torch.cuda.graph_pool_handle()) if pool is None else pool
    try:
        streams = (stream, *primed) if _workspace_cache_enabled() else ()
        for each in streams:
            _prime_library(device, each)
        prepare_pool(pool, stats(device)["allocated_peak"] if reserve_bytes is None else reserve_bytes, device)
        _ext._varena_working_pool_arm(device, cast(PoolId, tuple(pool)), [each.cuda_stream for each in streams])
        # The capture follows the caller's work on the device, and the host does not wait for it:
        # a recording overlaps the eager work before it. Unrelated device streams are never
        # drained. Native allocations during capture use this already prepared pool only.
        if stream != caller:
            stream.wait_stream(caller)
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
                routed = cast(ctypes.CDLL, _library).varena_torch_abort_routing(device, *pool)
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
    cuda_graph._varena_owner = (_owner, device, cast(PoolId, tuple(pool)))


@contextmanager
def replay_scope(device: int) -> Iterator[None]:
    """Exclude recovery around host launch only; acquire weight owners beforehand."""
    guard = _ext._VarenaAllocatorCapture(device, replay=True)
    try:
        yield
    finally:
        guard.close()


def replay(cuda_graph: Any, *, stream: Stream | None = None) -> None:
    """Replay an owned graph and retain its pool through completion on the selected stream.
    A scratch pool's contents are undefined between replays: copy every input in first."""
    _installed()
    import torch

    owner, device, pool = cuda_graph._varena_owner
    if owner != _owner:
        raise RuntimeError("a CUDA graph cannot be replayed after fork")
    stream = torch.cuda.current_stream(device) if stream is None else stream
    if _device(stream.device) != device:
        raise ValueError("replay stream belongs to a different device")
    prepare_pool(pool, 0, device)
    _ext._varena_working_pool_retain(device, pool)
    try:
        with torch.cuda.device(device), torch.cuda.stream(stream):
            try:
                cuda_graph.replay()
            finally:
                # End hooks may raise or reset the graph after launch. Our reference survives both.
                _ext._varena_working_pool_record_stream(device, pool, stream.cuda_stream, False)
    finally:
        _ext._varena_working_pool_release(device, pool)


def offload(model_or_pipeline, device=0, *, graphs="auto"):
    """Adopt immutable CPU weights: blocks bind real tensors, and loops become graphs.

    The modules keep the handle alive. See crates/varena/TORCH-OFFLOAD.md.
    """
    from ._offload import offload as adopt

    return adopt(model_or_pipeline, device, graphs=graphs)
