"""Gate hook, loaded by a package copy (cell.sh): this arm's `vt.checks`, and in the run record's
`execution.loops`, a digest of every component's outputs and each `denoise_loop`'s final tensors,
read in order after each call, and each split site's decisions (`split_trace`). It reads values; it changes none. `PRESS` changes only what memory the second
loop finds: "refill" fills the card once and lets go, so a kept program's weights left and come back, "hold" keeps it
filled through the loop, so they can't all come back and the call runs eagerly. "eager" turns graphs off: every loop
and component root runs as written."""

import hashlib
from typing import Any

import torch
from cozy_runtime import graphs
from cozy_runtime.internal import loop
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
_loops = [0]
_run, _dispatch = loop.Loops.run, graphs._dispatch


def _bytes(tensor: torch.Tensor) -> bytes:
    held = tensor.detach().cpu()
    return (held.to(torch.float64) if held.is_floating_point() else held).numpy().tobytes()


def _add(name: str, value: Any) -> None:
    request = _REQUEST.get()
    if request is None:
        return
    rows = loop.ROWS.setdefault(request.id, [])
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


def run(self: Any, step: Any, steps: Any, state: Any, carried: Any, inputs: Any, *rest: Any, **named: Any) -> None:
    _loops[0] += 1
    held = None
    if PRESS in ("refill", "hold") and _loops[0] == 2:
        device = state[0].device
        held = torch.empty(torch.cuda.mem_get_info(device)[1] - LEFT, dtype=torch.uint8, device=device)
        held.fill_(1)
        if PRESS == "refill":
            held = None
    _run(self, step, steps, state, carried, inputs, *rest, **named)
    del held
    _add("denoise_loop", state)


def dispatch(owner: Any, module: Any, call: Any, args: Any, kwargs: Any) -> Any:
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
loop.Loops.run, graphs._dispatch = run, dispatch
if PRESS == "eager":
    loop._eligible = lambda ctx, steps, tensors: ((), "the gate's eager arm")
    graphs._region_call = lambda torch, module, recorder, call, args, kwargs: call(module, *args, **kwargs)
