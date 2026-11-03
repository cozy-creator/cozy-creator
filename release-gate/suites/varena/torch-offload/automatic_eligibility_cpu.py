"""Actual cold-call aliases and pre-capture effect eligibility; no fake native owners."""

from __future__ import annotations

import functools
import gc
import json
import sys
import weakref
from pathlib import Path

import torch
from tensorfs.varena import _offload as adapter
from tensorfs.varena import torch as vt
from tensorfs.varena._offload_auto import (
    _cold_output_reason,
    _ForwardOrigin,
    _module_preflight,
    _promotion_reason,
)

torch.set_num_threads(1)
torch.set_num_interop_threads(1)


def adopt(model):
    # Actual callables before/after production adoption, not fake forward owners.
    originals = {id(child): child.forward for child in model.modules()}
    owner = vt.offload(model.eval(), device="cpu")
    origins = {
        id(child): _ForwardOrigin(child.forward, originals[id(child)])
        for child in model.modules()
    }
    return owner, origins


materializations, storage_queries = [], []


def classify(module, args, result):
    def profile(frame, event, value):
        if event == "call" and frame.f_code in (
            adapter._Version.tensor.__code__,
            adapter._Version.reservation.__code__,
        ):
            materializations.append(frame.f_code.co_name)
        if event == "c_call" and getattr(value, "__name__", "") == "_is_alias_of":
            storage_queries.append(True)

    previous = sys.getprofile()
    sys.setprofile(profile)
    try:
        return _cold_output_reason(module, args, {}, result)
    finally:
        sys.setprofile(previous)


class Outputs(torch.nn.Module):
    def __init__(self, kind):
        super().__init__()
        self.weight = torch.nn.Parameter(torch.arange(8.0))
        self.register_buffer("buffer", torch.arange(8.0))
        self.kind, self.calls = kind, 0

    def forward(self, value):
        self.calls += 1
        if self.kind == "input_view":
            return value, value.view(-1)
        if self.kind == "duplicate":
            result = value.clone()
            return result, result
        if self.kind == "disjoint_views":
            result = value.flatten().clone()
            return result[::2], result[1::2]
        if self.kind == "buffer":
            return self.buffer.view(2, 4)
        if self.kind == "parameter":
            return self.weight.detach()
        if self.kind == "empty_duplicate":
            result = value.new_empty(0)
            return result, result
        if self.kind == "empty_independent":
            return value.new_empty(0), value.new_empty(0)
        return value.clone(), value.clone()


expected = {
    "input_view": "output_aliases_input",
    "duplicate": "outputs_alias_each_other",
    "disjoint_views": "outputs_alias_each_other",
    "buffer": "output_aliases_buffer",
    "parameter": "output_aliases_managed_storage",
    "empty_duplicate": "outputs_alias_each_other",
    "empty_independent": None,
    "independent": None,
}
rows = []
for kind, reason in expected.items():
    model = Outputs(kind)
    owner, origins = adopt(model)
    preflight = _module_preflight(owner, model, forward_origins=origins)
    assert preflight == "unreviewed_module_effects" and model.calls == 0
    with torch.inference_mode():
        value = torch.arange(8.0).reshape(2, 4)
        result = model(value)  # The original cold call executes EXACTLY once.
        assert model.calls == 1
        assert (
            _promotion_reason(
                owner, model, (value,), {}, result, forward_origins=origins
            )
            is not None
        )
        actual = classify(model, (value,), result)
        assert actual == reason, (kind, actual)
        assert model.calls == 1
        if kind in ("duplicate", "empty_duplicate"):
            assert result[0] is result[1]
        if kind == "input_view":
            assert result[0] is value and torch._C._is_alias_of(result[1], value)
        if kind == "disjoint_views":
            assert result[0].data_ptr() != result[1].data_ptr()
            assert torch._C._is_alias_of(result[0], result[1])
        if kind == "empty_independent":
            assert result[0].data_ptr() == result[1].data_ptr() == 0
            assert not torch._C._is_alias_of(result[0], result[1])
        if kind == "independent":
            references = [weakref.ref(item) for item in result]
            del result
            gc.collect()
            assert all(reference() is None for reference in references)
    owner.close()
    rows.append({"case": kind, "reason": actual, "cold_body_calls": model.calls})

plain = torch.nn.Sequential(torch.nn.Linear(4, 4), torch.nn.ReLU()).eval()
owner, origins = adopt(plain)
assert _module_preflight(owner, plain, forward_origins=origins) is None
with torch.inference_mode():
    value = torch.ones(1, 4)
    assert classify(plain, (value,), plain(value)) is None
# Register a real public hook after adoption; don't infer purity from the earlier pass.
hook_calls = []
hook = plain[0].register_forward_hook(lambda *args: hook_calls.append(True))
assert (
    _module_preflight(owner, plain, forward_origins=origins)
    == "custom_module_hooks_require_adapter"
)
assert not hook_calls
with torch.inference_mode():
    plain(value)
assert hook_calls == [True]
hook.remove()
global_calls = []
global_hook = torch.nn.modules.module.register_module_forward_hook(
    lambda *args: global_calls.append(True)
)
try:
    assert (
        _module_preflight(owner, plain, forward_origins=origins)
        == "global_module_hooks_require_adapter"
    )
    assert not global_calls
finally:
    global_hook.remove()
owner.close()

identity = torch.nn.Identity()
owner, origins = adopt(identity)
assert _module_preflight(owner, identity, forward_origins=origins) is None
with torch.inference_mode():
    original_input = torch.arange(8.0)
    result = identity(original_input)
    assert result is original_input
    assert (
        _promotion_reason(
            owner, identity, (original_input,), {}, result, forward_origins=origins
        )
        == "output_aliases_input"
    )
    assert result is original_input
owner.close()

inplace = torch.nn.Sequential(torch.nn.Linear(4, 4), torch.nn.ReLU(inplace=True))
owner, origins = adopt(inplace)
assert _module_preflight(owner, inplace, forward_origins=origins) == "inplace_module"
owner.close()


class Mutable(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.register_buffer("counter", torch.zeros(1))
        self.effects = []

    def forward(self, value):
        self.counter.add_(1)
        self.effects.append("body")
        return value.clone()


mutable = Mutable()
owner, origins = adopt(mutable)
assert (
    _module_preflight(owner, mutable, forward_origins=origins)
    == "unreviewed_module_effects"
)
assert not mutable.effects and mutable.counter.item() == 0
with torch.inference_mode():
    result = mutable(value)
assert mutable.effects == ["body"] and mutable.counter.item() == 1
assert classify(mutable, (value,), result) is None
# Independent outputs and a successful cold pass do NOT make this module eligible.
assert (
    _module_preflight(owner, mutable, forward_origins=origins)
    == "unreviewed_module_effects"
)
assert mutable.effects == ["body"]
owner.close()

overridden = torch.nn.Linear(4, 4)
effects = []
original = overridden.forward


def changed(value):
    effects.append("body")
    return original(value)


overridden.forward = changed
owner, origins = adopt(overridden)
assert (
    _module_preflight(owner, overridden, forward_origins=origins)
    == "custom_forward_requires_adapter"
)
assert not effects
owner.close()

replaced = torch.nn.Linear(4, 4)
owner, origins = adopt(replaced)
installed = replaced.forward
replacement_effects = []


@functools.wraps(installed)
def wrapped(value):
    replacement_effects.append("body")
    return installed(value)


replaced.forward = wrapped
assert (
    _module_preflight(owner, replaced, forward_origins=origins)
    == "custom_forward_requires_adapter"
)
assert not replacement_effects
with torch.inference_mode():
    replaced(value)
assert replacement_effects == ["body"]
owner.close()
assert storage_queries and not materializations, materializations
assert not torch.cuda.is_initialized()
assert "libcuda.so" not in Path("/proc/self/maps").read_text()
print(
    json.dumps(
        {
            "torch": torch.__version__,
            "cuda": False,
            "actual_cold_output_controls": rows,
            "pre_capture_mutation_custom_forward_and_hook_refusal": True,
            "reviewed_standard_module_independent_output_control": True,
            "reviewed_identity_stays_eager_due_to_actual_output_alias": True,
            "classification_materializations": materializations,
            "actual_storage_metadata_queries": len(storage_queries),
            "classification_retains_no_output_tensors": True,
            "automatic_execution_wiring_cuda_or_alias_reconstruction": "not implemented/qualified here",
        }
    )
)
