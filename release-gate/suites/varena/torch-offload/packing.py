"""Real Torch semantics; --device 0 exercises native CUDA packing/refill.

No simulated native resources or injected SDK behavior. CPU mode qualifies only
actual host Tensor/Source behavior; native packing and retirement require CUDA.
"""

from __future__ import annotations

import argparse
import gc
import json

import torch
from tensorfs.varena import _offload as adapter
from tensorfs.varena import torch as vt

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
            dict.fromkeys(p._version_owner for p in module.parameters() if p.numel())
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

# Real public validation failures must preserve the adopted model and existing
# escaped versions. These do not claim an injected mid-publication failure.
before = [
    (
        p,
        p._version_owner,
        p._version_owner.region,
        p._version_owner.member_offset,
        p.clone(),
    )
    for p in model.parameters()
]
for conversion in (
    lambda: model.to(dtype=torch.int64),
    lambda: model.to(dtype=torch.float8_e4m3fn),
    lambda: model.to(
        device="cuda:0" if device.type == "cpu" else f"cuda:{(device.index or 0) + 1}"
    ),
):
    try:
        conversion()
    except (adapter.OffloadError, TypeError, ValueError):
        pass
    else:
        raise AssertionError("unsupported model conversion was accepted")
    for parameter, version, region, offset, value in before:
        assert parameter._version_owner is version
        assert version.region is region and version.member_offset == offset
        assert torch.equal(parameter.clone(), value)
    assert [id(p) for p in model.parameters()] == identities
    assert torch.equal(old.clone(), old_values)
checks.append(
    "real conversion refusals preserve identities, reservations and escaped values"
)

# A successful refresh exercises the actual native factory on CUDA. Group only
# the declared memberships and retain the existing old alias version independently.
model.double()
pending = [p._version_owner for p in model.parameters()]
assert all(v.region is None for v in pending)
expected_groups = adapter._unreserved_groups(definitions(owner))
with torch.no_grad():
    torch.testing.assert_close(
        model(inputs.to(device, dtype=torch.float64)).cpu(), expected.double()
    )
if device.type == "cuda":
    for group in expected_groups:
        assert len({id(version.region) for version in group}) == 1
        assert all(version.member_offset % 256 == 0 for version in group)
        assert len({version.member_offset for version in group}) == len(group)
assert torch.equal(old.clone(), old_values)
checks.append(
    "successful version refresh packs declared groups and preserves old aliases"
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
            "metadata_doubles": False,
            "pressure_qualification": "separate coordinator native pressure fixture required",
        }
    )
)
