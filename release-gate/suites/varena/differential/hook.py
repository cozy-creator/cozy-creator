"""Gate hook, loaded by a package copy (cell.sh): this arm's `vt.checks`, and in the run record's
`execution.loops`, a digest of every component's outputs, read in order after each call, and each split site's
decisions (`split_trace`). It reads values; it changes none. `PRESS` changes only what memory the second request
finds: "refill" fills the card once and lets go, so a kept graph's weights left and come back, "hold" keeps it filled
through the request, so they can't all come back and its calls run eagerly. "eager" turns graphs off: every component
root runs as written. Runtime 0.43 dropped `denoise_loop` and the executor's loop rows, so the hook keeps its own rows
and the executor's execution record carries them."""

import hashlib
from typing import Any

import torch

from cozy_runtime.internal.contract import unsealed

#: The driver's view, read on purpose: this cell tests the memory manager itself.
mem_get_info = unsealed(torch, "mem_get_info")
from cozy_runtime import graphs
from cozy_runtime.internal import executor
from cozy_runtime.internal.execution import catch
from cozy_runtime.internal.residency import _REQUEST
from torch.utils._pytree import tree_leaves

try:
    import tensorfs.varena.torch as vt
except ImportError:  # a v1 TensorFS has no checks: its base, cap, press and eager arms still digest
    vt = None

ARM: dict[str, bool] = {}  # cell.sh writes this arm's checks here
PRESS = ""  # and this arm's press
LEFT = 4 << 30  # less than SDXL's UNet: its weights can't all be here, and a block still fits

if vt is not None:
    vt.checks(**ARM)
_sums: dict[str, dict[str, Any]] = {}
_traces: dict[str, dict[str, list[list[Any]]]] = {}
_rows: dict[str, list[dict[str, Any]]] = {}  # each request's rows, until its execution record takes them
_requests: list[str] = []  # the requests seen, in order: the second one meets the press
_held: list[torch.Tensor] = []
_dispatch, _execution = graphs._dispatch, executor.Executor._execution


def _bytes(tensor: torch.Tensor) -> bytes:
    held = tensor.detach().cpu()
    return (held.to(torch.float64) if held.is_floating_point() else held).numpy().tobytes()


def _add(name: str, value: Any) -> None:
    request = _REQUEST.get()
    if request is None:
        return
    rows = _rows.setdefault(request.id, [])
    row = next((row for row in rows if "digests" in row), None)
    if row is None:
        row = {"steps": 0, "digests": {}, "split_trace": _traces.setdefault(request.id, {})}
        rows.append(row)
        _sums[request.id] = {}
    digest = _sums[request.id].setdefault(name, hashlib.sha256())
    for leaf in tree_leaves(value):
        if isinstance(leaf, torch.Tensor):
            digest.update(_bytes(leaf))
    row["digests"][name] = digest.hexdigest()
    row["checks"] = vt.check_counts() if vt is not None else {}


def _press(module: Any) -> None:
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


def execution(self: Any, *args: Any, **kwargs: Any) -> dict[str, Any]:
    """The executor's execution record, with this request's rows as `loops`; a held press lets go here."""
    _held.clear()
    document = _execution(self, *args, **kwargs)
    rows = [row for held in _rows.values() for row in held]
    _rows.clear()
    return {**document, "loops": rows} if rows else document


def dispatch(owner: Any, module: Any, call: Any, args: Any, kwargs: Any) -> Any:
    _press(module)
    result = _dispatch(owner, module, call, args, kwargs)
    if not (graphs.BYPASS.get() or graphs._WARMING.get() or torch.cuda.is_current_stream_capturing()):
        _add(graphs._name(module), result)
    return result


_select = getattr(catch, "select", None)


def select(device: int, key: str, menu: Any, stream: int, floor: int = 0) -> Any:
    """Each split decision in order, run-length: [level, basis, calls]. `declared` after a site has
    `measured` is a level that never ran (its trial); `refused` is the finest, chosen with no estimate."""
    level, selection = _select(device, key, menu, stream, floor)
    request = catch.CURRENT.get()
    if request is not None:
        trace = _traces.setdefault(request.id, {}).setdefault(key, [])
        mark = [level, selection["basis"] if selection else "refused"]
        if trace and trace[-1][:2] == mark:
            trace[-1][2] += 1
        else:
            trace.append([*mark, 1])
    return level, selection


if _select is not None:  # v2's native selection
    catch.select = select
graphs._dispatch, executor.Executor._execution = dispatch, execution
if PRESS == "eager":
    graphs._region_call = lambda torch, module, recorder, call, args, kwargs: call(module, *args, **kwargs)
