"""Real host tensors and adapter ownership; this never substitutes a CUDA object."""
from __future__ import annotations

import json
import threading
from types import SimpleNamespace
from pathlib import Path
import torch
from tensorfs.varena import torch as vt
from tensorfs.varena._offload import OffloadError
from tensorfs.varena._offload_stream import component_scope, reuse

assert not torch.cuda.is_initialized()
model = torch.nn.Sequential(torch.nn.Linear(4, 4), torch.nn.ReLU(), torch.nn.Linear(4, 2)).eval()
with torch.no_grad():
    for index, parameter in enumerate(model.parameters()):
        parameter.copy_(torch.arange(parameter.numel()).reshape(parameter.shape) / 32 + index / 8)
x = torch.arange(12, dtype=torch.float32).reshape(3, 4) / 16
with torch.inference_mode():
    expected = model(x).clone()
handle = vt.offload(model, device="cpu")
owner = handle._owner
assert not reuse(owner, None)
with torch.inference_mode(), component_scope(owner, (x,)) as outer:
    assert outer.stream is None and not outer.capture and not outer.reused
    assert reuse(owner, None)
    assert outer.completion is None
    live_checkpoint = outer.checkpoint()
    assert live_checkpoint.query() and not live_checkpoint.can_train
    assert reuse(owner, None), "checkpoint closed the still-active outer scope"
    cross_thread = []
    def wrong_thread():
        try:
            outer.checkpoint()
        except OffloadError:
            cross_thread.append("refused")
    thread = threading.Thread(target=wrong_thread)
    thread.start()
    thread.join()
    assert cross_thread == ["refused"]
    with component_scope(owner, (x,)) as inner:
        assert inner.reused and reuse(owner, None)
        value = inner.output(handle(x))
        measured = inner.checkpoint()
        measured.wait()
        assert measured.checked and reuse(owner, None)
    assert inner.completion.query() and not inner.completion.can_train
    result = {"value": value, "view": value.view(-1)}
    opaque = SimpleNamespace(sample=value)
    assert outer.output(opaque, values=(value,)) is opaque
    assert outer.output(result) is result
assert outer.completion.query() and not outer.completion.can_train
assert outer.completion.handoff(None, result) is result
assert not reuse(owner, None)
assert torch.equal(result["value"], expected)
assert result["view"].untyped_storage()._cdata == result["value"].untyped_storage()._cdata

try:
    with component_scope(owner, (x,)) as failed:
        assert reuse(owner, None)
        with torch.inference_mode():
            prefix = handle(x)
        prefix_completion = failed.checkpoint()
        prefix_completion.wait()
        assert torch.equal(prefix, expected)
        raise ValueError("actual body failure")
except ValueError as error:
    assert str(error) == "actual body failure"
else:
    raise AssertionError("component swallowed body failure")
assert not reuse(owner, None)
assert prefix_completion.checked and prefix_completion.query()
try:
    failed.completion.query()
except OffloadError:
    pass
else:
    raise AssertionError("failed scope claimed successful completion")

# Outside component entry, direct managed reads retain the existing actual path.
with torch.inference_mode():
    copied = model[0].weight.clone()
    assert copied.shape == (4, 4) and not reuse(owner, None)
handle.close()
assert not torch.cuda.is_initialized()
assert "libcuda.so" not in Path("/proc/self/maps").read_text()
print(json.dumps({"passed": True, "real_host_model": True, "nested_reuse": True,
                  "output_alias_preserved": True, "exception_context_restored": True,
                  "open_checkpoint_keeps_scope_active": True, "failed_witness_refused": True, "direct_weight_read": True,
                  "cuda_initialized": False, "native_cuda_library_loaded": False, "gpu_qualified": False}))
