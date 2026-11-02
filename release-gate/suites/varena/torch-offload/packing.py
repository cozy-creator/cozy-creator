"""Real CPU semantics; --device 0 prepares actual CUDA packing/refill checks.

CPU reservation doubles test only metadata transactions, never native behavior.
No model downloads. CUDA mode requires the matching region_parts native build.
"""

from __future__ import annotations

import argparse
import gc
import json
from types import SimpleNamespace
from unittest.mock import patch

import torch
from tensorfs.varena import torch as vt
from tensorfs.varena import _offload as adapter

parser = argparse.ArgumentParser()
parser.add_argument("--device", default="cpu")
args = parser.parse_args()
torch.set_num_threads(1)
torch.set_num_interop_threads(1)
checks = []


class Block(torch.nn.Module):
    def __init__(self, shared, alias):
        super().__init__()
        self.weight = torch.nn.Parameter(torch.arange(1.0, 5.0))
        self.bias = torch.nn.Parameter(torch.arange(4.0))
        self.shared = shared
        self.alias = torch.nn.Parameter(alias)

    def forward(self, value):
        return value * self.weight + self.bias + self.shared + self.alias


class Model(torch.nn.Module):
    _no_split_modules = ("Block",)

    def __init__(self):
        super().__init__()
        storage = torch.arange(8.0)
        shared = torch.nn.Parameter(storage[::2])
        self.blocks = torch.nn.ModuleList(
            [Block(shared, storage[1::2]), Block(shared, storage[1::2])]
        )
        self.unclassified = torch.nn.Parameter(torch.ones(4))

    def forward(self, value):
        for block in self.blocks:
            value = block(value)
        return value + self.unclassified


def definitions(owner):
    return {
        name: list(
            dict.fromkeys(
                p._version_owner for p in module.parameters() if p.numel()
            )
        )
        for name, module in owner._boundary_modules.items()
    }


def refused(function):
    try:
        function()
    except (adapter.OffloadError, RuntimeError, ValueError):
        return
    raise AssertionError("out-of-storage alias accepted")


model = Model().eval()
inputs = torch.arange(4.0)
with torch.no_grad():
    expected = model(inputs)
identities = [id(p) for p in model.parameters()]
owner = vt.offload(model, device="cpu" if args.device == "cpu" else int(args.device))
device = owner._owner.device
left, right = model.blocks
shared = left.shared._version_owner
assert shared is right.shared._version_owner is left.alias._version_owner
assert shared is right.alias._version_owner
assert left.shared is right.shared
groups = adapter._unreserved_groups(definitions(owner))
assert sorted(map(len, groups)) == [1, 2, 2]
assert all(model.unclassified._version_owner not in group for group in groups)
assert next(group for group in groups if shared in group) == [shared]
checks.append("exact existing membership sets union ties and physical aliases")

# A direct read before the first intent permanently reserves a singleton on CUDA.
direct = left.weight.clone()
direct_version = left.weight._version_owner
direct_region = direct_version.region
torch.testing.assert_close(direct.cpu(), torch.arange(1.0, 5.0))
with torch.no_grad():
    actual = model(inputs.to(device))
torch.testing.assert_close(actual.cpu(), expected)
assert left.weight._version_owner is direct_version
if device.type == "cuda":
    assert direct_region is not None and direct_version.region is direct_region
    assert direct_version.member_offset == 0
    assert right.weight._version_owner.region is right.bias._version_owner.region
    assert (
        right.weight._version_owner.member_offset
        != right.bias._version_owner.member_offset
    )
    assert shared.region is not right.weight._version_owner.region
    assert owner.intent_facts()["regions"] == {
        "model.blocks.0": 3,
        "model.blocks.1": 2,
    }
    assert model.unclassified._version_owner.region is not shared.region
checks.append("direct reads and unclassified storage retain singleton reservations")

# Packed neighbors must never become addressable through storage-local aliases.
refused(lambda: torch.as_strided(right.weight, (5,), (1,)))
refused(lambda: torch.as_strided(right.weight, (1,), (1,), storage_offset=4))
refused(lambda: right.weight._version_owner.tensor((5,), (1,), 0, None))
assert torch.equal(right.weight.clone().cpu(), torch.arange(1.0, 5.0))
checks.append("alias and export spans cannot escape their original storage")

old = left.shared.detach()
old_values = old.clone()
old_right_alias = right.alias._version_owner
left.half()
assert left.shared is right.shared and left.shared.dtype == torch.float16
assert left.shared._version_owner is left.alias._version_owner
assert right.alias._version_owner is old_right_alias is shared
assert old.dtype == torch.float32 and torch.equal(old.clone(), old_values)
assert left.shared._version_owner.region is None
model.float()
assert [id(p) for p in model.parameters()] == identities
assert torch.equal(old.clone(), old_values)
with torch.no_grad():
    torch.testing.assert_close(model(inputs.to(device)).cpu(), expected)
checks.append("partial half/float keeps tied identity and escaped old versions")

# Real CPU/Torch rebinding rollback: fail once after one identity was rebound.
before = [(p, p._version_owner, p.clone()) for p in model.parameters()]
rebind = adapter._rebind
calls = 0


def fail_second(parameter, candidate):
    global calls
    calls += 1
    if calls == 2:
        raise RuntimeError("injected conversion publication failure")
    rebind(parameter, candidate)


with patch.object(adapter, "_rebind", fail_second):
    try:
        model.double()
    except RuntimeError as error:
        assert str(error) == "injected conversion publication failure"
    else:
        raise AssertionError("conversion failure was swallowed")
for parameter, version, value in before:
    assert parameter._version_owner is version and torch.equal(parameter.clone(), value)
checks.append("failed conversion restores real Parameter versions and values")

# Check reservation transaction failure after an earlier group was staged. CPU
# uses inert handles solely to inspect publication; CUDA wraps the real factory.
model.double()
pending = [p._version_owner for p in model.parameters()]
assert all(v.region is None for v in pending)
created = []
real_gpu = owner._owner.gpu


class MetadataRegion:
    closed = False

    def defer_close(self):
        self.closed = True


def fail_group(parts):
    if created:
        raise RuntimeError("injected grouped-source construction failure")
    if real_gpu is None:
        region = MetadataRegion()
        offsets = (0,) * len(parts)  # Never exported or used as layout evidence.
    else:
        region, offsets = factory(parts)
    created.append(region)
    return region, offsets


factory = None if real_gpu is None else real_gpu.region_parts
fake = SimpleNamespace(region_parts=fail_group)
try:
    if real_gpu is None:
        owner._owner.gpu = fake
        with owner._owner.lock:
            try:
                owner._refresh_intent()
            except RuntimeError as error:
                assert str(error) == "injected grouped-source construction failure"
            else:
                raise AssertionError("group failure was swallowed")
    else:
        with patch.object(real_gpu, "region_parts", fail_group), owner._owner.lock:
            try:
                owner._refresh_intent()
            except RuntimeError as error:
                assert str(error) == "injected grouped-source construction failure"
            else:
                raise AssertionError("group failure was swallowed")
finally:
    owner._owner.gpu = real_gpu
assert created and all(v.region is None and v.member_offset == 0 for v in pending)
assert created[0].closed if real_gpu is None else created[0]._handle == 0
assert torch.equal(old.clone(), old_values)
with torch.no_grad():
    torch.testing.assert_close(
        model(inputs.to(device, dtype=torch.float64)).cpu(), expected.double()
    )
checks.append(
    "group construction failure publishes no partial layout; old versions remain readable"
)

if device.type == "cuda":
    # Refill after explicit native trim; no fake pressure or forced OOM loop.
    del actual, direct, old_values, before
    gc.collect()
    torch.cuda.synchronize(device)
    resident = owner._owner.gpu.stats()["weights_resident"]
    owner._owner.gpu.trim()
    trimmed = owner._owner.gpu.stats()["weights_resident"]
    assert trimmed < resident, (resident, trimmed)
    with torch.no_grad():
        refill = model(inputs.to(device, dtype=torch.float64))
    torch.testing.assert_close(refill.cpu(), expected.double())
    checks.append(
        "actual native trim releases packed weights and refill preserves values"
    )
else:
    assert not torch.cuda.is_initialized()

owner.close()
assert torch.equal(old.clone().cpu(), torch.arange(8.0)[::2])
print(
    json.dumps(
        {
            "torch": torch.__version__,
            "device": str(device),
            "checks": checks,
            "native_executed": device.type == "cuda",
            "metadata_doubles": device.type == "cpu",
            "pressure_qualification": "separate coordinator native pressure fixture required",
        }
    )
)
