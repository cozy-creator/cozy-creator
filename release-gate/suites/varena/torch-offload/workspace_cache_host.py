"""Real shim accessor/latching/managed eligibility; no CUDA device is initialized.

Run with the matching TensorFS wheel and Torch installed. Each setting gets a
fresh process. GPU GraphSegment capture and automatic body execution are separate
qualification boundaries; this client exercises their shared native-state check.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys


def child(setting):
    import torch
    from tensorfs.varena import torch as vt

    expected = setting == "1"
    expected_reason = "cached_library_workspace" if expected else None
    assert not torch.cuda.is_initialized()
    vt._bind_allocator()  # Real same-cohort binding; do not activate a device.
    first_reason = vt._managed_graph_refusal()
    first_state = vt._workspace_cache_enabled()
    assert first_state is expected
    assert first_reason == expected_reason

    # An environment proxy would now give the wrong answer. Torch's first query
    # has latched the actual process state, including when the variable was unset.
    changed = "0" if expected else "1"
    os.environ["TORCH_CUBLAS_WORKSPACE_CACHE"] = changed
    assert vt._workspace_cache_enabled() is expected
    assert vt._managed_graph_refusal() == expected_reason
    assert not torch.cuda.is_initialized()
    assert not any(
        "libcuda.so" in line
        for line in Path("/proc/self/maps").read_text().splitlines()
    )
    print(json.dumps({
        "initial_setting": setting,
        "native_state": first_state,
        "managed_graph_refusal": first_reason,
        "changed_environment": changed,
        "latched_state_preserved": True,
        "cuda_initialized": False,
        "cuda_driver_loaded": False,
        "torch": torch.__version__,
        "torch_git": torch.version.git_version,
        "gpu_capture_and_automatic_body": "not exercised",
    }))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--child", choices=("unset", "0", "1"))
    args = parser.parse_args()
    if args.child is not None:
        child(args.child)
        return
    receipts = []
    for setting in ("unset", "0", "1"):
        env = os.environ.copy()
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        if setting == "unset":
            env.pop("TORCH_CUBLAS_WORKSPACE_CACHE", None)
        else:
            env["TORCH_CUBLAS_WORKSPACE_CACHE"] = setting
        result = subprocess.run(
            [sys.executable, "-B", str(Path(__file__).resolve()), "--child", setting],
            env=env, capture_output=True, text=True,
        )
        if result.stderr:
            print(result.stderr, file=sys.stderr, end="")
        result.check_returncode()
        receipts.append(json.loads(result.stdout))
    print(json.dumps({"workspace_cache_host": "pass", "children": receipts}))


if __name__ == "__main__":
    main()
