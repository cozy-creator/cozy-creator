import hashlib
import json
from pathlib import Path

import torch
from cozy_runtime.author import _split
from cozy_runtime.internal import execution
from cozy_runtime.internal.execution import catch, cuts
from diffusers.models.attention import FeedForward
from torch.utils._python_dispatch import TorchDispatchMode


class Census(TorchDispatchMode):
    def __init__(self):
        self.calls = []

    def __torch_dispatch__(self, func, types, args=(), kwargs=None):
        self.calls.append(str(func))
        return func(*args, **(kwargs or {}))


torch.set_num_threads(1)
torch.manual_seed(138)
rows = []
with torch.no_grad():
    for module, value in (
        (torch.nn.Conv3d(3, 4, 3, padding=1).eval(), torch.randn(2, 3, 4, 5, 6)),
        (torch.nn.RMSNorm(32).eval(), torch.randn(3, 7, 32)),
        (
            FeedForward(32, dim_out=32, activation_fn="gelu").eval(),
            torch.randn(2, 7, 32),
        ),
    ):
        expected = module(value)
        cuts.install(module)
        actual = module(value)
        torch.testing.assert_close(actual, expected, rtol=0, atol=0)
        rows.append({"module": type(module).__qualname__, "exact": True})
    _split.install(execution)
    value = torch.linspace(-1, 1, 18).view(6, 3)
    for most, count in ((None, 1), (6, 3)):
        with Census() as census:
            actual = _split.split(torch.sin, value, _split.Cut(0), most=most)
        torch.testing.assert_close(actual, torch.sin(value), rtol=0, atol=0)
        assert census.calls.count("aten.sin.default") == count, census.calls
        rows.append({"authored_most": most, "actual_sin_calls": count, "exact": True})
assert not hasattr(catch, "cut")
assert not catch.uncommitted_scope()
assert (
    not torch.cuda.is_initialized()
    and "libcuda.so" not in Path("/proc/self/maps").read_text()
)
print(
    json.dumps(
        {
            "passed": True,
            "torch": torch.__version__,
            "torch_commit": torch.version.git_version,
            "scope": "actual CPU native-ineligible ordinary operations and authored fixed windows, no fake provider",
            "GPU_qualified": False,
            "runtime_source_sha256": hashlib.sha256(
                Path(cuts.__file__).read_bytes()
            ).hexdigest(),
            "cases": rows,
        },
        indent=2,
    )
)
