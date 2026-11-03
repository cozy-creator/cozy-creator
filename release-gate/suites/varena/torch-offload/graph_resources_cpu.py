"""Real adopted CPU model + installed Torch retained-holder/finalizer collection.

No CUDAGraph, CUDA initialization, fake native Region/Plan or GPU-lifetime claim.
The shortened actual owner back-edge reproduces the retained Definition defect.
"""

from __future__ import annotations

import gc
import json
import weakref

import torch
from tensorfs.varena import torch as vt
from tensorfs.varena._offload_graph import GraphSegment, _Definition, _retained_payload
from tensorfs.varena.sources import Source
from torch.cuda.graphs import _RetainedCallbacks


def tracked_sources():
    return {id(value) for value in gc.get_objects() if type(value) is Source}


def arm():
    model = torch.nn.Sequential(torch.nn.Linear(4, 4), torch.nn.ReLU()).eval()
    owner = vt.offload(model, device="cpu")
    value = torch.arange(8.0).reshape(2, 4)
    with torch.no_grad():
        result = model(value)
    versions = tuple(dict.fromkeys(p._version_owner for p in model.parameters()))
    definition = _Definition(
        owner,
        model,
        versions,
        (),
        None,
        "cpu-collection-only",
        None,
        GraphSegment._parameters(model),
        GraphSegment._buffers(model),
        None,
        static_tree=((value,), {}),
        outputs=result,
    )
    # Real Tensor metadata can hold a control object. Payload storage ownership
    # must not retain this wrapper/tree as a hidden owner back-reference.
    result.control_owner = owner
    payload = _retained_payload(definition)
    holder = _RetainedCallbacks()  # Actual installed Torch implementation.
    holder.objects[1] = payload
    target = torch.empty(1)  # Real independent CPU object; NOT a stand-in graph.
    finalizer = weakref.finalize(target, holder.fire)
    return {
        "owner": weakref.ref(owner),
        "model": weakref.ref(model),
        "versions": [weakref.ref(version) for version in versions],
        "source_ids": {id(version.source) for version in versions},
        "original_result": weakref.ref(result),
        "device_owner": weakref.ref(owner._owner),
        "payload": weakref.ref(payload),
        "expected": result.clone(),
        "target": target,
        "holder": holder,
        "finalizer": finalizer,
    }


if __name__ == "__main__":
    torch.set_num_threads(1)
    torch.set_num_interop_threads(1)
    refs = arm()
    gc.collect()
    assert refs["owner"]() is None, (
        "graph-retained payload kept the real Offload owner alive"
    )
    assert refs["model"]() is None
    assert refs["original_result"]() is None
    assert all(value() is not None for value in refs["versions"])
    assert refs["device_owner"]() is not None
    assert refs["finalizer"].alive and not refs["holder"]._fired
    assert refs["source_ids"] <= tracked_sources()
    payload = refs["payload"]()
    assert payload is not None
    assert payload.versions == tuple(value() for value in refs["versions"])
    assert not hasattr(payload, "owner") and not hasattr(payload, "module")
    assert all(
        not tensor.__dict__ and tensor._base is None for tensor in payload.working
    )
    torch.testing.assert_close(payload.working[-1], refs["expected"], rtol=0, atol=0)
    working_refs = [weakref.ref(tensor) for tensor in payload.working]
    del payload
    del refs["target"]
    gc.collect()
    assert not refs["finalizer"].alive and refs["holder"]._fired
    assert not refs["holder"].objects
    gc.collect()
    assert not (refs["source_ids"] & tracked_sources())
    assert all(value() is None for value in refs["versions"])
    assert refs["device_owner"]() is None
    assert refs["payload"]() is None and all(value() is None for value in working_refs)
    assert not torch.cuda.is_initialized()
    print(
        json.dumps(
            {
                "passed": True,
                "scope": "real CPU Offload +Torch retained-holder finalizer; NO CUDAGraph or GPU simulation",
                "owner_model_versions_sources_collected": True,
                "ordinary_tensor_backref_not_retained": True,
                "resources_stay_live_without_control_owner_until_holder_fires": True,
                "exact_versions_and_device_custody_retained_until_holder_fires": True,
                "torch_cuda_initialized": False,
            }
        )
    )
