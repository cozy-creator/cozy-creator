"""Real SDK CPU refusal/profile client; successful CUDA returns remain GPU-only."""

import argparse
import json
import sys
from pathlib import Path

import torch
from capture_pressure_evidence import ReturnEvidence
from tensorfs.varena import _offload_auto as auto
from tensorfs.varena import torch as vt


def collector(context):
    return ReturnEvidence(
        vt.prepare_pool,
        auto._Observation.close,
        vt.phase_stats,
        context,
        capture_begin=torch.cuda.CUDAGraph.capture_begin,
        capture_complete=vt.capture_complete,
        graph_scope=vt.graph,
        replay_scope=vt.replay_scope,
    )


def prepare_refusal():
    try:
        vt.prepare_pool((0, 0), 2097152, 0)
    except RuntimeError as error:
        return {"type": type(error).__name__, "message": str(error)}
    raise AssertionError("uninstalled SDK preparation unexpectedly succeeded")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    assert not torch.cuda.is_initialized()
    assert sys.getprofile() is None
    state = {
        "active": True,
        "boundary": "host_refusal",
        "attempt": 1,
        "device": 0,
        "stage": 4,
    }

    def context():
        return state if state["active"] else None

    evidence = collector(context)
    evidence.install()
    try:
        refusal = prepare_refusal()
        facts = evidence.read("capture-pressure-host", 0)
        assert facts["available"]
        # Actual native host replay exclusion stays held despite stage4/inactive.
        with vt.replay_scope(0):
            assert prepare_refusal() == refusal
            state["active"] = False
            blocked = evidence.read("capture-pressure-host", 0)
            assert not blocked["available"]
            assert blocked["events"] is blocked["stored_native_samples"] is None
            evidence.report_unavailable()
        state["active"] = True
        assert prepare_refusal() == refusal
        resumed = evidence.read("capture-pressure-host", 0)
        assert resumed["available"] and len(resumed["events"]) == 2
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
    uncertain = collector(context)
    uncertain.install()
    try:
        try:
            with vt.replay_scope(0):
                # A real native public refusal propagates through the actual scope.
                vt.prepare_pool((0, 0), 2097152, 0)
        except RuntimeError as error:
            assert {"type": type(error).__name__, "message": str(error)} == refusal
        else:
            raise AssertionError("actual refusal was lost")
        state["active"] = False
        unavailable = uncertain.read("capture-pressure-host", 0)
        assert not unavailable["available"]
        # Later ordinary scope success must not erase earlier unknown completion.
        with vt.replay_scope(0):
            pass
        assert not uncertain.read("capture-pressure-host", 0)["available"]
    finally:
        uncertain.stop()
    assert not uncertain.read("capture-pressure-host", 0)["available"]
    assert sys.getprofile() is None and not torch.cuda.is_initialized()
    result = {
        "passed": True,
        "python": sys.version,
        "torch": torch.__version__,
        "torch_git": torch.version.git_version,
        "cuda_initialized": False,
        "scope": "actual SDK preparation/phase_stats refusals and real native host replay scopes; no fake resources; CUDA uninitialized",
        "refusal": refusal,
        "evidence": facts,
        "stage4_inactive_open_scope_unavailable": blocked,
        "normal_scope_exit_resumes_collection": resumed,
        "exception_scope_exit_stays_unavailable": unavailable,
        "capture_complete_and_retained_CUDA_exclusion": "UNRUN; requires real GPU capture",
        "successful_prepare_return": "UNRUN; requires actual CUDA pool",
        "successful_phase_close_and_stored_sample": "UNRUN; requires actual CUDA observation",
        "model_and_capture": "UNRUN",
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
