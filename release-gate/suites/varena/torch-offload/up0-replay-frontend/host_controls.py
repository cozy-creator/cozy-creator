"""Real CPU profiling controls; no model, CUDA resource, mock or native core build."""

import argparse
import ctypes
import gc
import hashlib
import json
import os
from pathlib import Path
import sys
import threading
import weakref

import torch
from tensorfs.varena import torch as vt
from tensorfs.varena import _offload_auto as auto, _offload_graph as graph, api
from frontend_trace import FrontendTrace, bind_native, returns


def normal_none():
    return None


def raised_error():
    raise ValueError("actual host control exception")


def cpu_copy(refs):
    source, target = torch.arange(4), torch.empty(4, dtype=torch.int64)
    refs.extend((weakref.ref(source), weakref.ref(target)))
    target.copy_(source)
    assert target.tolist() == [0, 1, 2, 3]
    try:
        target.copy_(torch.empty(3))
    except RuntimeError:
        pass
    else:
        raise AssertionError("real CPU copy shape error did not occur")


def cpu_clone(refs):
    tensor = torch.arange(4)
    cloned = tensor.clone()
    refs.extend((weakref.ref(tensor), weakref.ref(cloned)))
    assert cloned.tolist() == [0, 1, 2, 3]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--helper", type=Path, required=True)
    parser.add_argument("--helper-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    assert hashlib.sha256(args.helper.read_bytes()).hexdigest() == args.helper_sha256
    assert hashlib.sha256(Path(auto.__file__).read_bytes()).hexdigest() == "831608fe961047c75103a10f8c3514f4157c20d3f25df9555ed148a4490d21be"
    assert hashlib.sha256(Path(graph.__file__).read_bytes()).hexdigest() == "2fc6fd464505d57ba3441b694586b2b648539b7dc63b97a938ba2b90c61eb27e"
    assert torch.__version__ == "2.15.0+cu132"
    assert not torch.cuda.is_initialized()
    vt._bind_allocator()
    native = ctypes.CDLL(str(args.helper), mode=os.RTLD_NOW | os.RTLD_LOCAL | os.RTLD_NODELETE)
    bind_native(native)
    workspace_cache = native.allocation_trace_workspace_cache_enabled()
    assert workspace_cache == 0
    assert native.allocation_trace_install() == 0
    trace = FrontendTrace(native, torch, vt, auto, graph, api, args.output,
                          {"scope": "actual CPU host controls only; model unrun"})
    # Register the real host functions as additional observed code identities.
    # None of the original functions, native APIs or lifecycle resources is replaced.
    trace.codes[normal_none.__code__] = 110
    trace.codes[raised_error.__code__] = 120
    for code in (normal_none.__code__, raised_error.__code__):
        trace.return_offsets[code] = returns(code)
    trace.copy_codes.add(cpu_copy.__code__)
    trace.clone_codes.add(cpu_clone.__code__)
    trace.boundary = "host_controls_no_graph"
    trace.dispatch_frame = -1  # Scalar host-control selection, not a fabricated frame.
    trace.invocation = 0
    assert native.allocation_trace_window(1) == 0
    trace.window = True
    trace.set_stage(4)
    trace.install()
    refs = []
    thread_profiles = []
    try:
        normal_none()
        try:
            raised_error()
        except ValueError as error:
            assert str(error) == "actual host control exception"
        cpu_copy(refs)
        cpu_clone(refs)
        before_thread = len(trace.host_rows)

        def other_thread():
            thread_profiles.append(sys.getprofile() is None)
            normal_none()

        thread = threading.Thread(target=other_thread)
        thread.start()
        thread.join()
        assert len(trace.host_rows) == before_thread and thread_profiles == [True]
        trace.set_stage(0)
        before_gap = json.loads(native.allocation_trace_dump())
        normal_none()  # Real scalar marker remains recorded with ATen disabled.
        assert torch.add(torch.arange(4), 1).tolist() == [1, 2, 3, 4]
        after_gap = json.loads(native.allocation_trace_dump())
        fields = {name: i for i, name in enumerate(after_gap["row_fields"])}
        gap = after_gap["rows"][len(before_gap["rows"]):]
        assert len(gap) == 2 and all(row[fields["kind"]] == 17 for row in gap)
        # Exercise the real public API's invalid-device refusal when reading stats.
        # This is not a mocked phase_stats function or a CUDA resource.
        trace._keys.add((-1, "host_control_invalid_device"))
        trace.save("stats-refusal")
        persisted = json.loads((args.output / "up0-trace-0-stats-refusal.returns.json").read_text())
        assert persisted["stored_native_samples"][0]["sample_error"]
        assert (args.output / "up0-trace-0-stats-refusal.native.json").is_file()
    finally:
        trace.dispatch_frame = None
        trace.finish()
    gc.collect()
    assert all(ref() is None for ref in refs), "profiler retained a Tensor or its frame"
    facts = json.loads((args.output / "up0-trace-0-process-end.native.json").read_text())
    assert not facts["lost"] and not trace.host_errors, trace.host_errors
    markers = [row["marker"] for row in trace.host_rows]
    assert markers == [110, 111, 120, 122, 130, 131, 130, 132, 150, 151, 110, 111], markers
    assert not any(row[fields["kind"]] in (15, 16) for row in facts["rows"])
    assert sys.getprofile() is None
    assert not torch.cuda.is_initialized()
    assert "libcuda.so" not in Path("/proc/self/maps").read_text()
    print(json.dumps({"passed": True, "markers": markers, "normal_none_classified": True,
                      "real_python_error_classified": True, "real_cpu_copy_error_classified": True,
                      "cpu_tensor_weakrefs_released": len(refs), "stats_refusal_trace_survives": True,
                      "unprofiled_thread_proven": True, "gap_native_markers_without_ATen": True,
                      "cuda_initialized": False, "cuda_driver_loaded": False,
                      "workspace_cache_actual_native": workspace_cache,
                      "execution_base": "original9d-c059 SDK; Runtime wheel not needed by these CPU controls",
                      "model_dispatch_and_Rust_callbacks": "UNRUN; not emulated",
                      "scope": "host profile mechanism and exact helper only"}, indent=2))


if __name__ == "__main__":
    main()
