

# Appended for diff-writes (cell.sh): each denoise_loop's step, `before` and `after` also bump counters outside the loop's
# tensors, through a torch op and a raw Triton launch, and each call's record holds the counts against how often running
# without graphs would bump them. `IDLE`: the first call records nothing, so the idle period after it records the call.
import triton  # noqa: E402
import triton.language as tl  # noqa: E402

IDLE = False  # cell.sh writes this arm's


@triton.jit
def _bump(pointer):
    tl.store(pointer, tl.load(pointer) + 1)


_counted: dict[str, torch.Tensor] = {}
_want = {"step": 0, "before": 0, "after": 0}
_counting_run, _record_first, _recorded = loop.Loops.run, loop._record_first, [False]


def _counting(part: str, call: Any) -> Any:
    def counted(*args: Any, **kwargs: Any) -> None:
        call(*args, **kwargs)
        _counted[f"{part}_torch"].add_(1)
        _bump[(1,)](_counted[f"{part}_triton"])

    return counted


def _writes_run(self: Any, step: Any, steps: Any, state: Any, carried: Any, inputs: Any, phases: Any, key: Any,
                on_step: Any, before: Any, after: Any, cache: Any = None) -> None:
    for part in _want:  # made eagerly, before any capture could reach them
        for way in ("torch", "triton"):
            _counted.setdefault(f"{part}_{way}", torch.zeros(1, dtype=torch.int64, device=state[0].device))
    _want["step"] += len(steps)
    _want["before"] += before is not None
    _want["after"] += after is not None
    _counting_run(self, _counting("step", step), steps, state, carried, inputs, phases, key, on_step,
                  before and _counting("before", before), after and _counting("after", after), cache)
    torch.cuda.synchronize(state[0].device)
    if (request := _REQUEST.get()) is not None:
        counts = {name: int(tensor.item()) for name, tensor in _counted.items()}
        loop.ROWS.setdefault(request.id, []).append({"writes": counts, "want": dict(_want)})


def _first_records_nothing(*args: Any, **kwargs: Any) -> Any:
    if not _recorded[0]:
        _recorded[0] = True
        return None, "", 0
    return _record_first(*args, **kwargs)


loop.Loops.run = _writes_run
if IDLE:
    loop._record_first = _first_records_nothing
