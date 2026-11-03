"""Real host helper/subscriber checks; no fabricated allocator event or CUDA work."""
from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import re
import threading

import torch
from tensorfs import _ext
from tensorfs.varena import api
from tensorfs.varena import torch as vt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--helper", type=Path, required=True)
    parser.add_argument("--mode", choices=("markers", "overflow", "workspace"), required=True)
    parser.add_argument("--initial-cache-state", choices=("unset", "0", "1"))
    args = parser.parse_args()
    assert torch.__version__ == "2.15.0+cu132"
    assert torch.version.git_version == "d0f2af8b66b20da4baa50644c0cd3f887a2eae2e"
    assert not torch.cuda.is_initialized()
    assert api.core_identity() == _ext._varena_core_identity()
    native = ctypes.CDLL(str(args.helper), mode=os.RTLD_NOW | os.RTLD_LOCAL | os.RTLD_NODELETE)
    native.allocation_trace_install.restype = ctypes.c_int
    native.allocation_trace_window.argtypes = [ctypes.c_int]
    native.allocation_trace_window.restype = ctypes.c_int
    native.allocation_trace_stage.argtypes = [ctypes.c_int]
    native.allocation_trace_stage.restype = None
    native.allocation_trace_marker.argtypes = [ctypes.c_uint32, ctypes.c_uint32, ctypes.c_int32] + [ctypes.c_uint64] * 5
    native.allocation_trace_marker.restype = ctypes.c_uint64
    native.allocation_trace_dump.restype = ctypes.c_char_p
    native.allocation_trace_error.restype = ctypes.c_char_p
    native.allocation_trace_workspace_cache_enabled.argtypes = []
    native.allocation_trace_workspace_cache_enabled.restype = ctypes.c_int
    initial_setting = os.environ.get("TORCH_CUBLAS_WORKSPACE_CACHE")
    before_query = json.loads(native.allocation_trace_dump())
    assert not before_query["observer_attached"]
    workspace_cache = native.allocation_trace_workspace_cache_enabled()
    assert workspace_cache in (0, 1), native.allocation_trace_error().decode(errors="replace")
    assert os.environ.get("TORCH_CUBLAS_WORKSPACE_CACHE") == initial_setting
    if args.mode == "workspace":
        assert args.initial_cache_state is not None
        expected_setting = None if args.initial_cache_state == "unset" else args.initial_cache_state
        assert initial_setting == expected_setting
        assert workspace_cache == int(initial_setting == "1")
        # This host control changes its own environment AFTER the first native read;
        # the diagnostic helper itself neither reads Python env nor writes any env.
        changed_setting = "0" if workspace_cache else "1"
        os.environ["TORCH_CUBLAS_WORKSPACE_CACHE"] = changed_setting
        repeated = native.allocation_trace_workspace_cache_enabled()
        assert repeated == workspace_cache
        assert os.environ["TORCH_CUBLAS_WORKSPACE_CACHE"] == changed_setting
        assert not json.loads(native.allocation_trace_dump())["observer_attached"]
        assert not torch.cuda.is_initialized()
        maps = Path("/proc/self/maps").read_text()
        assert "libcuda.so" not in maps
        native_paths = sorted({line.split()[-1] for line in maps.splitlines()
                               if line.endswith("/libvarena.so") or "/libstd-" in line})
        print(json.dumps({
            "passed": True, "mode": "workspace", "initial_setting": args.initial_cache_state,
            "scope": "actual Torch native getter and first-call latching; no allocator installation",
            "native_state": bool(workspace_cache), "after_env_change_state": bool(repeated),
            "changed_setting": changed_setting, "observer_registered": False,
            "cuda_initialized": False, "cuda_driver_loaded": False,
            "native_payload_sha256": {Path(p).name: hashlib.sha256(Path(p).read_bytes()).hexdigest() for p in native_paths},
            "model_execution": "UNRUN",
        }, indent=2))
        return
    vt._bind_allocator()  # Actual CPU-only binding; never install()/initialize(device).
    assert native.allocation_trace_install() == 0
    before = json.loads(native.allocation_trace_dump())
    assert before["observer_attached"] and not before["lost"]
    assert not before["rows"] and before["segment_free_semantics"] == "pre_unmap_retirement_intent"
    header = Path(__file__).with_name("diagnostic_markers.h")
    expected = {value: name for name, value in re.findall(r"XS138_(\w+)\s*=\s*(\d+)", header.read_text())}
    assert before["marker_names"] == expected
    assert native.allocation_trace_window(1) == 0

    def mark(marker, invocation=1):
        # No model/graph/device was created: unavailable identities are explicit0/-1.
        return native.allocation_trace_marker(marker, invocation, -1, 0, 0, 0, 0, 0)

    ordinals = []
    if args.mode == "markers":
        native.allocation_trace_stage(4)
        ordinals.append(mark(100))
        assert torch.add(torch.arange(4), 1).tolist() == [1, 2, 3, 4]
        ordinals.append(mark(101))
        native.allocation_trace_stage(0)
        # Real native marker calls from two host threads share the committed ring.
        def host_span(invocation):
            ordinals.append(mark(100, invocation))
            assert sum(range(4)) == 6
            ordinals.append(mark(101, invocation))
        threads = [threading.Thread(target=host_span, args=(i,)) for i in (2, 3)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
        assert native.allocation_trace_window(0) == 0
        facts = json.loads(native.allocation_trace_dump())
        assert not facts["lost"] and facts["dropped_rows"] == 0
        fields = {name: index for index, name in enumerate(facts["row_fields"])}
        rows = facts["rows"]
        assert len(rows) == facts["snapshot_cut_ordinal"]
        assert all(value > 0 for value in ordinals) and len(set(ordinals)) == len(ordinals)
        markers = [row for row in rows if row[fields["kind"]] == 17]
        assert len(markers) == 6
        assert all(rows[value - 1][fields["kind"]] == 17 for value in ordinals)
        assert len({row[fields["thread"]] for row in markers}) >= 2
        times = [row[fields["monotonic_ns"]] for row in rows]
        assert times == sorted(times) and all(times)
        assert any(row[fields["kind"]] == 9 for row in rows)  # Actual CPU ATen execution.
        assert not any(row[fields["kind"]] in (7, 11, 15, 16) for row in rows)
        assert all(row[fields["stage"]] == 0 for row in markers if row[fields["invocation"]] in (2, 3))
        assert mark(100, 4) == 0
        after = json.loads(native.allocation_trace_dump())
        assert after["rows"] == rows and after["inactive_markers"] == 1
    else:
        capacity = before["row_capacity"]
        for _ in range(capacity):
            assert mark(100) != 0
        assert mark(101) == 0
        assert native.allocation_trace_window(0) == 0
        facts = json.loads(native.allocation_trace_dump())
        assert facts["lost"] and facts["dropped_rows"] == 1
        assert len(facts["rows"]) == capacity

    assert not torch.cuda.is_initialized()
    maps = Path("/proc/self/maps").read_text()
    assert "libcuda.so" not in maps
    native_paths = sorted({line.split()[-1] for line in maps.splitlines()
                           if line.endswith("/libvarena.so") or "/libstd-" in line})
    print(json.dumps({
        "passed": True, "mode": args.mode, "scope": "real host marker/subscriber API and CPU ATen only",
        "observer_registered": facts["observer_attached"], "rows": len(facts["rows"]),
        "lost": facts["lost"], "dropped_rows": facts["dropped_rows"],
        "workspace_cache_actual_native": bool(workspace_cache),
        "cuda_initialized": False, "cuda_driver_loaded": False,
        "native_payload_sha256": {Path(p).name: hashlib.sha256(Path(p).read_bytes()).hexdigest() for p in native_paths},
        "identity_unavailable_reasons": {"device_stream_graph_plan_pool": "host check creates none"},
        "GPU_segment_events": "UNRUN; none synthesized", "model_execution": "UNRUN",
    }, indent=2))


if __name__ == "__main__":
    main()
