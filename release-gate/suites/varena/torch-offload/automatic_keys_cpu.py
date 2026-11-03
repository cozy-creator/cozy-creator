"""Actual CPU module boundary/key integration; no native graph substitute."""

from __future__ import annotations

import gc
import json
import weakref
from pathlib import Path

import torch
from tensorfs.varena import torch as vt
from tensorfs.varena._offload_auto import _boundary_frontier, _InvocationKey

torch.set_num_threads(1)
torch.set_num_interop_threads(1)


class Block(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.layers = torch.nn.Sequential(torch.nn.Linear(4, 4), torch.nn.Linear(4, 4))

    def forward(self, value):
        return self.layers(value)


model = torch.nn.Sequential(Block(), Block()).eval()
with torch.no_grad():
    reference = model(torch.ones(1, 4))
owner = vt.offload(model, device="cpu")
frontier, reasons = _boundary_frontier(owner)
assert [boundary.name for boundary in frontier] == ["model.0", "model.1"]
assert any(
    reason == "selected_ancestor_owns_this_invocation" for reason in reasons.values()
)
boundary = frontier[0]
with torch.no_grad():
    torch.testing.assert_close(model(torch.ones(1, 4)), reference, rtol=0, atol=0)
with torch.inference_mode():
    value = torch.ones(1, 4)
    value_ref = weakref.ref(value)
    key = _InvocationKey.from_call(owner, boundary, (value,), {})
    same = _InvocationKey.from_call(owner, boundary, (value.clone(),), {})
    assert key == same and hash(key) == hash(same)
    assert key.native_key(id(owner)) == same.native_key(id(owner))
    changed_shape = _InvocationKey.from_call(owner, boundary, (torch.ones(2, 4),), {})
    assert key != changed_shape
    value.add_(3)  # Dynamic input values must not change shape/config selection.
    assert key == _InvocationKey.from_call(owner, boundary, (value,), {})
    del value
    gc.collect()
    assert value_ref() is None, "key retained a transient input Tensor"
    mapping_key = torch.ones(1)
    mapping_key_ref = weakref.ref(mapping_key)
    try:
        _InvocationKey.from_call(
            owner, boundary, ({mapping_key: torch.ones(1, 4)},), {}
        )
    except RuntimeError as error:
        assert "mapping keys" in str(error)
    else:
        raise AssertionError("input TreeSpec retained an ordinary Tensor mapping key")
    del mapping_key
    gc.collect()
    assert mapping_key_ref() is None
    old = boundary.module.layers[0].weight.detach()
    old_values = old.clone()
    boundary.module.double()
    changed_dtype = _InvocationKey.from_call(
        owner, boundary, (torch.ones(1, 4, dtype=torch.float64),), {}
    )
    assert key != changed_dtype and key.versions != changed_dtype.versions
    torch.testing.assert_close(old.clone(), old_values, rtol=0, atol=0)
    assert all(version.region is None for version in key.versions)
owner.close()


class Composite(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.first = torch.nn.Linear(4, 4)
        self.second = torch.nn.Linear(4, 4)

    def forward(self, value):
        return self.second(self.first(value))


composite_owner = vt.offload(Composite().eval(), device="cpu")
selected, declined = _boundary_frontier(composite_owner)
assert (
    not selected and declined["model"] == "composite_root_requires_segment_boundaries"
)
composite_owner.close()
shared = torch.nn.Linear(4, 4)
shared_model = torch.nn.Sequential(shared, shared).eval()
with torch.no_grad():
    shared_reference = shared_model(torch.ones(1, 4))
shared_owner = vt.offload(shared_model, device="cpu")
shared_frontier, shared_reasons = _boundary_frontier(shared_owner)
assert len(shared_frontier) == 1 and shared_frontier[0].name == "model.0"
assert shared_reasons["model.1"] == "shared_module_uses_its_existing_boundary"
with torch.no_grad():
    torch.testing.assert_close(
        shared_model(torch.ones(1, 4)), shared_reference, rtol=0, atol=0
    )
shared_owner.close()
conv = torch.nn.Conv2d(1, 1, 3).eval()
conv_owner = vt.offload(conv, device="cpu")
conv_boundary = _boundary_frontier(conv_owner)[0][0]
with torch.no_grad():
    no_grad_key = _InvocationKey.from_call(
        conv_owner, conv_boundary, (torch.ones(1, 1, 8, 8),), {}
    )
with torch.inference_mode():
    x = torch.ones(1, 1, 8, 8)
    original = _InvocationKey.from_call(conv_owner, conv_boundary, (x,), {})
    assert original != no_grad_key
    conv.stride = (2, 2)
    assert original != _InvocationKey.from_call(conv_owner, conv_boundary, (x,), {})
    conv.stride = (1, 1)
    assert original == _InvocationKey.from_call(conv_owner, conv_boundary, (x,), {})
conv_owner.close()
assert not torch.cuda.is_initialized()
assert "libcuda.so" not in Path("/proc/self/maps").read_text()
print(
    json.dumps(
        {
            "torch": torch.__version__,
            "cuda": False,
            "nonoverlapping_existing_boundary_frontier": True,
            "composite_root_not_promoted_to_full_model_graph": True,
            "shared_module_has_one_boundary_and_exact_eager_values": True,
            "immutable_shape_dtype_config_and_inference_keys": True,
            "transient_input_not_retained": True,
            "tensor_mapping_key_refused_without_hidden_treespec_ownership": True,
            "old_versions_alias_values_preserved_without_reservation": True,
            "automatic_execution_and_native_cache_pressure": "not implemented or qualified here",
        }
    )
)
