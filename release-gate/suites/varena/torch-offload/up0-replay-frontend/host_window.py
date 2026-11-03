"""Exercise original automatic dispatch/window using an actually adopted CPU pipeline."""

import argparse
from collections import OrderedDict
import ctypes
import gc
import hashlib
import json
import os
from pathlib import Path
import sys
import weakref

import torch
from torch import nn
from tensorfs.varena import torch as vt
from tensorfs.varena import _offload_auto as auto, _offload_graph as graph, api
from frontend_trace import FrontendTrace, bind_native


class Pipeline:
    def __init__(self):
        self.unet = nn.Sequential(OrderedDict([
            ("up_blocks", nn.Sequential(nn.Linear(4, 4), nn.Linear(4, 4))),
        ])).eval()
        self.components = {"unet": self.unet}

    def __call__(self, value):
        return self.unet(value)


def run_pipeline(trace):
    pipeline = Pipeline()
    with torch.no_grad():
        for layer in pipeline.unet.up_blocks:
            layer.weight.copy_(torch.eye(4) * 2)
            layer.bias.fill_(1)
        value = torch.arange(4, dtype=torch.float32).reshape(1, 4)
        expected = pipeline(value)
    owner = vt.offload(pipeline, device="cpu", graphs="auto")
    assert set(owner._boundary_modules) == {"unet.up_blocks.0", "unet.up_blocks.1"}
    assert {state.boundary.name for state in owner._automatic._states.values()} == {
        "unet.up_blocks.0", "unet.up_blocks.1"}
    refs = [weakref.ref(owner), weakref.ref(owner._automatic), weakref.ref(pipeline),
            weakref.ref(pipeline.unet), weakref.ref(value), weakref.ref(expected)]
    calls = {"0": 0, "1": 0}

    def count(name):
        def completed(_module, _inputs, _output):
            calls[name] += 1
        return completed

    hooks = [layer.register_forward_hook(count(str(index)))
             for index, layer in enumerate(pipeline.unet.up_blocks)]
    original_dispatch = auto._AutomaticGraphs.dispatch
    trace.install()
    try:
        with torch.no_grad():
            for _ in range(3):
                result = pipeline(value)
                assert torch.equal(result, expected)
                refs.append(weakref.ref(result))
        counters = {state.boundary.name: state.eager_calls
                    for state in owner._automatic._states.values()}
        assert counters == {"unet.up_blocks.0": 3, "unet.up_blocks.1": 3}
        assert calls == {"0": 3, "1": 3}
        assert auto._AutomaticGraphs.dispatch is original_dispatch
        assert trace.complete and not trace.window and trace.invocation == 3
    finally:
        trace.finish()
        for hook in hooks:
            hook.remove()
        owner.close()
    return refs, calls, counters


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--helper", type=Path, required=True)
    parser.add_argument("--helper-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    assert hashlib.sha256(args.helper.read_bytes()).hexdigest() == args.helper_sha256
    assert hashlib.sha256(Path(auto.__file__).read_bytes()).hexdigest() == "831608fe961047c75103a10f8c3514f4157c20d3f25df9555ed148a4490d21be"
    assert torch.__version__ == "2.15.0+cu132"
    assert not torch.cuda.is_initialized()
    vt._bind_allocator()
    native = ctypes.CDLL(str(args.helper), mode=os.RTLD_NOW | os.RTLD_LOCAL | os.RTLD_NODELETE)
    bind_native(native)
    assert native.allocation_trace_workspace_cache_enabled() == 0
    assert native.allocation_trace_install() == 0
    trace = FrontendTrace(native, torch, vt, auto, graph, api, args.output,
                          {"scope": "real adopted CPU pipeline; original dispatch/window only"})
    refs, calls, counters = run_pipeline(trace)
    gc.collect()
    assert all(reference() is None for reference in refs), "profiler retained owner/module/Tensor/frame"
    assert not trace.host_errors, trace.host_errors
    facts = json.loads((args.output / "up0-trace-3-process-end.native.json").read_text())
    assert not facts["lost"]
    fields = {name: index for index, name in enumerate(facts["row_fields"])}
    marker_rows = [row for row in facts["rows"] if row[fields["kind"]] == 17]
    assert [row[fields["marker"]] for row in marker_rows] == [100, 101, 100, 101, 170, 171]
    assert [row[fields["invocation"]] for row in marker_rows] == [2, 2, 3, 3, 3, 3]
    assert [row["boundary"] for row in trace.host_rows] == [
        "unet.up_blocks.0", "unet.up_blocks.0", "unet.up_blocks.0", "unet.up_blocks.0",
        "unet.up_blocks.1", "unet.up_blocks.1"]
    assert all(row[fields["device"]] == -1 and row[fields["stream"]] == 0
               and row[fields["graph_identity"]] == row[fields["plan_handle"]] == 0
               for row in marker_rows)
    assert all(row["identity_unavailable_reason"] for row in trace.host_rows)
    assert all(row[fields["kind"]] not in (15, 16) for row in facts["rows"])
    assert sys.getprofile() is None and not torch.cuda.is_initialized()
    assert "libcuda.so" not in Path("/proc/self/maps").read_text()
    print(json.dumps({"passed": True, "scope": "actual CPU adoption and ORIGINAL automatic dispatch/window",
                      "exactly_once_body_counts": calls, "original_dispatch_eager_counts": counters,
                      "outputs_equal": True, "selected_invocations": [2, 3],
                      "following_boundary": "unet.up_blocks.1", "trace_complete": trace.complete,
                      "released_owner_module_tensor_weakrefs": len(refs),
                      "unknown_CPU_device_stream": [-1, 0], "markers": [row["marker"] for row in trace.host_rows],
                      "cuda_initialized": False, "cuda_driver_loaded": False,
                      "GPU_capture_replay_and_Rust_callbacks": "UNRUN; none emulated"}, indent=2))


if __name__ == "__main__":
    main()
