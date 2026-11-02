"""Metadata-only reservation transaction checks. No native allocation proof."""

from __future__ import annotations

import json

import torch
from tensorfs.varena import torch as vt

torch.set_num_threads(1)
torch.set_num_interop_threads(1)


class Region:
    closed = False

    def defer_close(self):
        self.closed = True


class Plan(Region):
    def close(self):
        self.closed = True


class Factory:
    def __init__(self):
        self.created = []
        self.fail_plan = True

    def region_parts(self, sources):
        region = Region()
        self.created.append(region)
        return region, tuple(256 * i for i in range(len(sources)))

    def plan(self, definitions):
        self.definitions = definitions
        if self.fail_plan:
            raise RuntimeError("injected Plan construction failure")
        return Plan()


model = torch.nn.Sequential(torch.nn.Linear(2, 2), torch.nn.Linear(2, 2)).eval()
owner = vt.offload(model, device="cpu")
versions = [p._version_owner for p in model.parameters()]
reserved = versions[0]
reserved.region = Region()
original_region = reserved.region
old_plan = Plan()
owner._intent_plan = old_plan
factory = Factory()
owner._owner.gpu = factory
with owner._owner.lock:
    try:
        owner._refresh_intent()
    except RuntimeError as error:
        assert str(error) == "injected Plan construction failure"
    else:
        raise AssertionError("Plan failure was swallowed")
assert owner._intent_plan is old_plan and not old_plan.closed
assert owner._intent_dirty
assert reserved.region is original_region and not original_region.closed
assert all(v.region is None and v.member_offset == 0 for v in versions[1:])
assert len(factory.created) == 2 and all(r.closed for r in factory.created)

factory.fail_plan = False
with owner._owner.lock:
    owner._refresh_intent()
assert old_plan.closed and owner._intent_plan is not old_plan
assert reserved.region is original_region and reserved.member_offset == 0
assert versions[2].region is versions[3].region
assert versions[2].member_offset == 0 and versions[3].member_offset == 256
assert owner.intent_facts()["regions"] == {"model.0": 2, "model.1": 1}
assert len(factory.definitions["model.1"]) == 1
assert not owner._intent_dirty

owner._owner.gpu = None
owner._intent_plan = None
owner.close()
assert not torch.cuda.is_initialized()
print(
    json.dumps(
        {
            "torch": torch.__version__,
            "native_executed": False,
            "metadata_doubles": True,
            "checks": [
                "Plan failure closes all staged Regions without partial publication",
                "published singleton survives failed and successful refresh",
                "successful refresh publishes member offsets and deduplicates Plan Regions",
            ],
        }
    )
)
