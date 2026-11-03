"""Real SDK CPU refusal/profile client; successful CUDA returns remain GPU-only."""

import argparse
import json
import sys
from pathlib import Path

import torch
from capture_pressure_evidence import ReturnEvidence
from tensorfs.varena import _offload_auto as auto
from tensorfs.varena import torch as vt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    assert not torch.cuda.is_initialized()
    assert sys.getprofile() is None
    evidence = ReturnEvidence(
        vt.prepare_pool,
        auto._Observation.close,
        vt.phase_stats,
        lambda: {"boundary": "host_refusal", "attempt": 1, "device": 0, "stage": 4},
    )
    evidence.install()
    try:
        try:
            vt.prepare_pool((0, 0), 2097152, 0)
        except RuntimeError as error:
            refusal = {"type": type(error).__name__, "message": str(error)}
        else:
            raise AssertionError("uninstalled SDK preparation unexpectedly succeeded")
        facts = evidence.read("capture-pressure-host", 0)
    finally:
        evidence.stop()
    assert sys.getprofile() is None
    assert not torch.cuda.is_initialized()
    assert not facts["errors"]
    assert len(facts["events"]) == 1
    row = facts["events"][0]
    assert row["kind"] == "prepare_pool_return"
    assert row["pool"] == (0, 0) and row["device"] == row["requested_device"] == 0
    assert row["requested_bytes"] == 2097152
    assert not row["returned_capacity_available"]
    assert row["returned_capacity_bytes"] is None
    assert facts["stored_native_samples"] == [
        {
            "device": 0,
            "native_key": "capture-pressure-host",
            "sample_error": "RuntimeError",
        }
    ]
    result = {
        "passed": True,
        "torch": torch.__version__,
        "torch_git": torch.version.git_version,
        "cuda_initialized": False,
        "scope": "actual public prepare_pool pre-install refusal observed without replacing it; actual phase_stats refusal; profile removed",
        "refusal": refusal,
        "evidence": facts,
        "successful_prepare_return": "UNRUN; requires actual CUDA pool",
        "successful_phase_close_and_stored_sample": "UNRUN; requires actual CUDA observation",
        "model_and_capture": "UNRUN",
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
