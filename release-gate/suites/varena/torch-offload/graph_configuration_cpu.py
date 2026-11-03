"""Real Conv2d configuration/output controls; no fabricated graph or native owner."""

from __future__ import annotations

import copy
import json
import weakref
from pathlib import Path

import torch
from tensorfs.varena import torch as vt
from tensorfs.varena._offload_graph import GraphSegment

torch.set_num_threads(1)
torch.set_num_interop_threads(1)


def run(attribute, replacement, *, nested=False):
    conv = torch.nn.Conv2d(1, 1, 3, stride=1, padding=0, dilation=1, bias=False)
    with torch.no_grad():
        conv.weight.fill_(0.125)
    model = (torch.nn.Sequential(conv) if nested else conv).eval()
    reference = copy.deepcopy(model)
    reference_conv = reference[0] if nested else reference
    owner = vt.offload(model, device="cpu")
    value = torch.arange(64.0).reshape(1, 1, 8, 8)
    captured_key = GraphSegment._configuration(model)
    parameters = GraphSegment._parameters(model)
    buffers = GraphSegment._buffers(model)
    original = getattr(conv, attribute)
    with torch.inference_mode():
        before = model(value)
        torch.testing.assert_close(before, reference(value), rtol=0, atol=0)
        setattr(conv, attribute, replacement)
        setattr(reference_conv, attribute, replacement)
        changed_key = GraphSegment._configuration(model)
        after = model(value)
        torch.testing.assert_close(after, reference(value), rtol=0, atol=0)
        assert before.shape != after.shape
        assert GraphSegment._parameters(model) == parameters
        assert GraphSegment._buffers(model) == buffers
        # Restoring the exact tuple must recover the frozen metadata key.
        setattr(conv, attribute, original)
        assert GraphSegment._configuration(model) == captured_key
        # A configuration snapshot must not acquire ownership of unsupported
        # tuples containing real Tensor storage or mutable objects.
        ordinary = torch.ones(1)
        weak = weakref.ref(ordinary)
        conv._fixture_opaque_tuple = (ordinary, [])
        ignored_key = GraphSegment._configuration(model)
        assert ignored_key == captured_key
        del conv._fixture_opaque_tuple, ordinary
        assert weak() is None
    owner.close()
    return {
        "attribute": attribute,
        "nested": nested,
        "original": original,
        "replacement": replacement,
        "captured_output_shape": list(before.shape),
        "current_output_shape": list(after.shape),
        "configuration_key_changed": captured_key != changed_key,
        "parameter_buffer_state_unchanged": True,
        "actual_eager_output_matches_reference": True,
        "restoring_configuration_restores_key": True,
        "unsupported_tensor_mutable_tuple_not_retained": True,
    }


rows = [
    run("stride", (2, 2)),
    run("padding", (1, 1)),
    run("dilation", (2, 2)),
    run("stride", (2, 2), nested=True),
]
assert not torch.cuda.is_initialized()
assert "libcuda.so" not in Path("/proc/self/maps").read_text()
print(
    json.dumps(
        {
            "torch": torch.__version__,
            "cuda": False,
            "actual_conv_configuration_controls": rows,
            "stale_cuda_replay": "not exercised; coordinator GPU fixture required",
        }
    ),
    flush=True,
)
assert all(row["configuration_key_changed"] for row in rows)
