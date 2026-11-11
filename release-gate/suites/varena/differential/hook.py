"""Gate hook, loaded by a package copy (cell.sh): this arm's `vt.checks`, and in the run record's
`execution.loops`, a digest of every component's outputs, read in order after each call, and each split site's
decisions (`split_trace`). It reads values; it changes none. `PRESS` changes only what memory the second request
finds: "refill" fills the card once and lets go, so a kept graph's weights left and come back, "hold" keeps it filled
through the request, so they can't all come back and its calls run eagerly. "eager" turns graphs off: every component
root runs as written. Runtime 0.43 dropped `denoise_loop` and the executor's loop rows, so the hook keeps its own rows
and the executor's execution record carries them."""

import hashlib
from collections.abc import Callable, Mapping, Sequence
from types import ModuleType
from typing import TYPE_CHECKING, Protocol, TypedDict, cast

import torch

from cozy_runtime.internal.contract import unsealed

#: The driver's view, read on purpose: this cell tests the memory manager itself.
mem_get_info = cast(Callable[[torch.device], tuple[int, int]], unsealed(torch, "mem_get_info"))  # torch.cuda.mem_get_info
from cozy_runtime import graphs
from cozy_runtime.author._errors import DoesNotFit
from cozy_runtime.internal import executor
from cozy_runtime.internal.execution import catch
from cozy_runtime.internal.residency import _REQUEST
from torch.utils._pytree import tree_leaves

if TYPE_CHECKING:
    from tensorfs.varena._facts import SplitSelection


class _Checks(Protocol):
    """What the hook calls of `tensorfs.varena.torch`."""

    def checks(self, *, unmap_idle: bool = False, perturb: bool = False, guards: bool = False) -> None: ...

    def check_counts(self) -> dict[str, int]: ...


vt: _Checks | None
try:
    import tensorfs.varena.torch as varena_torch

    vt = varena_torch
except ImportError:  # a v1 TensorFS has no checks: its base, cap, press and eager arms still digest
    vt = None

ARM: dict[str, bool] = {}  # cell.sh writes this arm's checks here
PRESS = ""  # and this arm's press
LEFT = 4 << 30  # less than SDXL's UNet: its weights can't all be here, and a block still fits

if vt is not None:
    vt.checks(**ARM)
#: A split site's run of one decision: [level, basis, calls].
type Mark = list[str | int]
type Menu = Sequence[tuple[str, Sequence[int]]]


class Row(TypedDict, total=False):
    steps: int
    digests: dict[str, str]
    split_trace: dict[str, list[Mark]]
    checks: dict[str, int]


class Call(Protocol):
    """A component root's call, as the Runtime hands it to `_dispatch`."""

    def __call__(self, module: torch.nn.Module, /, *args: object, **kwargs: object) -> object: ...


#: The native selection this hook was written for: (level, selection), or no selection when refused.
type Select = Callable[[int, str, str, Menu, int], "SplitSelection"]

_sums: dict[str, dict[str, "hashlib._Hash"]] = {}
_traces: dict[str, dict[str, list[Mark]]] = {}
_rows: dict[str, list[Row]] = {}  # each request's rows, until its execution record takes them
_requests: list[str] = []  # the requests seen, in order: the second one meets the press
_held: list[torch.Tensor] = []
_dispatch, _execution = graphs._dispatch, executor.Executor._execution


def _bytes(tensor: torch.Tensor) -> bytes:
    held = tensor.detach().cpu()
    return (held.to(torch.float64) if held.is_floating_point() else held).numpy().tobytes()


def _add(name: str, value: object) -> None:
    request = _REQUEST.get()
    if request is None:
        return
    rows = _rows.setdefault(request.id, [])
    row = next((row for row in rows if "digests" in row), None)
    if row is None:
        row = Row(steps=0, digests={}, split_trace=_traces.setdefault(request.id, {}))
        rows.append(row)
        _sums[request.id] = {}
    digest = _sums[request.id].setdefault(name, hashlib.sha256())
    for leaf in tree_leaves(value):
        if isinstance(leaf, torch.Tensor):
            digest.update(_bytes(leaf))
    row["digests"][name] = digest.hexdigest()
    row["checks"] = vt.check_counts() if vt is not None else {}


def _press(module: torch.nn.Module) -> None:
    """Before the second request's first component call: fill the card, and let go ("refill") or keep it ("hold")."""
    request = _REQUEST.get()
    if request is None or request.id in _requests:
        return
    _requests.append(request.id)
    if PRESS in ("refill", "hold") and len(_requests) == 2:
        device = next(module.parameters()).device
        held = torch.empty(mem_get_info(device)[1] - LEFT, dtype=torch.uint8, device=device)
        held.fill_(1)
        if PRESS == "hold":
            _held.append(held)


type Record = dict[str, object]


def _with_loops[**P](record: Callable[P, Record]) -> Callable[P, Record]:
    def execution(*args: P.args, **kwargs: P.kwargs) -> Record:
        """The executor's execution record (`record(self, ...)`), with this request's rows as `loops`; a held press
        lets go here."""
        _held.clear()
        document = record(*args, **kwargs)
        rows = [row for held in _rows.values() for row in held]
        _rows.clear()
        return {**document, "loops": rows} if rows else document

    return execution


def dispatch(owner: ModuleType, module: torch.nn.Module, call: Call, args: tuple[object, ...], kwargs: dict[str, object]) -> object:
    """`_dispatch` as Runtime 0.44 has it: `owner` is the torch module."""
    _press(module)
    result: object = _dispatch(owner, module, call, args, kwargs)
    if not (graphs.BYPASS.get() or graphs._WARMING.get() or torch.cuda.is_current_stream_capturing()):
        _add(graphs._name(module), result)
    return result


_select: Select | None = getattr(catch, "select", None)


def _mark(key: str, level: int, basis: str) -> None:
    request = catch.CURRENT.get()
    if request is not None:
        trace = _traces.setdefault(request.id, {}).setdefault(key, [])
        mark: Mark = [level, basis]
        if trace and trace[-1][:2] == mark:
            trace[-1][2] = cast(int, trace[-1][2]) + 1  # a mark's third item is its count
        else:
            trace.append([*mark, 1])


def select(device: int, key: str, name: str, menu: Menu, stream: int) -> "SplitSelection":
    """Each split decision in order, run-length: [level, basis, calls]. `declared` after a site has
    `measured` is a level that never ran (its trial); `refused` is the finest, which native selection
    could not make room for (the call then fails with DoesNotFit). Runtime 0.44's `catch.select`."""
    assert _select is not None  # installed only where it exists
    try:
        selection = _select(device, key, name, menu, stream)
    except DoesNotFit:
        _mark(key, len(menu) - 1, "refused")
        raise
    _mark(key, int(selection["choice_id"]), selection["basis"])
    return selection


if _select is not None:  # v2's native selection
    catch.select = select
graphs._dispatch = dispatch  # type: ignore[assignment]  # graphs' own is untyped (Any): this one names the call's type
executor.Executor._execution = _with_loops(_execution)  # type: ignore[method-assign]  # the hook wraps the executor's record
if PRESS == "eager":
    graphs._region_call = lambda torch, module, recorder, call, args, kwargs: call(module, *args, **kwargs)
