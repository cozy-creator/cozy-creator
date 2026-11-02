"""CPU-only namespace, stream-signature and Runtime terminal-refusal proof.

Run with the composed SDK and Runtime on sys.path. This checks host contracts only;
it does not qualify CUDA execution, asynchronous Source or native retirement.
"""

from __future__ import annotations

import hashlib
import inspect
import json
import sys
from pathlib import Path

from tensorfs import _ext, varena
from tensorfs.varena import _runtime, api


def main() -> None:
    assert "torch" not in sys.modules, "public/private imports initialized Torch"
    assert all(getattr(varena, name) is getattr(api, name) for name in varena.__all__)
    old = ("Arena", "Cooperation", "CooperationOwner", "CacheOwner", "LoadSlot",
           "make_room", "capture", "replay")
    assert not any(hasattr(varena, name) for name in old)
    assert not hasattr(_runtime, "Cache"), "a second Python Cache API survived"
    assert hasattr(_ext, "_VarenaArenaRetirement")
    assert hasattr(_ext._VarenaArena, "close")
    for method, arguments in (
        (varena.Gpu.use, (None, [])),
        (varena.Gpu.alloc, (None, 1)),
        (varena.CacheView.release, (None, 0, 1)),
    ):
        assert inspect.signature(method).parameters["stream"].default is inspect.Parameter.empty
        try:
            method(*arguments)
        except TypeError:
            pass
        else:
            raise AssertionError(f"{method.__qualname__} accepted an omitted stream")

    import torch
    from cozy_runtime.author._errors import Fault, fault
    from cozy_runtime.internal import residency
    from cozy_runtime.internal.execution import catch

    assert not torch.cuda.is_initialized()
    error = varena.Exhausted("CPU typed-refusal contract", 123)
    assert fault(error) is Fault.EXHAUSTED
    assert error.varena_no_retry and not catch.short(error)
    invocations = []

    def run(level):
        invocations.append(level)
        raise error

    memory = residency.Memory.open(torch.device("cpu"))
    with memory.request("canonical-exhausted", plan=[]) as request, catch.serving(request):
        try:
            catch.cut("canonical.exhausted", 2, lambda _level, _granularity: 0, run)
        except varena.Exhausted as caught:
            assert caught is error
        else:
            raise AssertionError("terminal exhaustion was swallowed")
    assert invocations == [0], invocations
    assert not torch.cuda.is_initialized()
    print(json.dumps({
        "passed": True,
        "scope": "CPU namespace, required streams, native interface visibility and terminal refusal",
        "native_extension": _ext.__file__,
        "native_sha256": hashlib.sha256(Path(_ext.__file__).read_bytes()).hexdigest(),
        "public_exports": varena.__all__,
        "fault": fault(error).value,
        "terminal_cut_invocations": invocations,
        "cuda_initialized": False,
        "gpu_execution_qualified": False,
    }, indent=2))


if __name__ == "__main__":
    main()
