"""Real GPU reused-footprint refusal; invoke through the operator's Cozy child runner.

G1 captures actual Working storage. Rejected G2 must not retire G1's footprint.
template-only defers G1 instantiation until after G2 reset/drop. Prepared/unrun;
metadata retirement is not a claim of physical GPU release. No provider needed.
"""

import argparse
import gc
import hashlib
import json
import weakref
from pathlib import Path

import torch
from tensorfs.varena import torch as vt

HELPER_SHA256 = "9fc6130efbf0a6341bd0e77e8133bcdf9873438e3662d89930ffaa9fbe321777"
REUSED_FOOTPRINT = (
    "invalid residency operation: graph footprint is one capture definition"
)


def same_live_footprint(footprint, expected):
    facts = footprint.facts()
    assert facts == expected, (expected, facts)
    assert facts["sealed"] and not facts["retired"]
    return facts


def replay_exact(graph, working, expected, stream):
    with vt.replay_scope(stream.device.index):
        vt.replay(graph, stream=stream)
    stream.synchronize()
    torch.testing.assert_close(working.cpu(), expected, rtol=0, atol=0)


def run(args):
    helper_sha = hashlib.sha256(Path(vt.__file__).read_bytes()).hexdigest()
    assert helper_sha == args.expected_helper_sha256, (
        helper_sha,
        args.expected_helper_sha256,
    )
    assert not torch.cuda.is_initialized(), "requires the normal fresh child"
    vt.install(args.device)
    device = torch.device("cuda", args.device)
    actual_uuid = str(torch.cuda.get_device_properties(device).uuid)
    assert (
        actual_uuid.removeprefix("GPU-").lower()
        == args.gpu_uuid.removeprefix("GPU-").lower()
    )
    stream = torch.cuda.Stream(device=device)
    pool = tuple(torch.cuda.graph_pool_handle())
    footprint = vt.capture_footprint(args.device, pool)
    reserve = 4 * 1024**2
    host = torch.arange(512 * 1024, dtype=torch.float32)
    rejected_body_entered = False

    with torch.inference_mode(), torch.cuda.stream(stream):
        source = host.to(device)
        warm = torch.empty_like(source)
        warm.copy_(source)
        stream.synchronize()
        del warm

        g1 = torch.cuda.CUDAGraph(keep_graph=True)
        with vt.graph(
            g1, pool=pool, stream=stream, reserve_bytes=reserve, footprint=footprint
        ):
            working = torch.empty_like(source)  # Actual allocation inside capture.
            working.copy_(source)
        captured = footprint.facts()
        assert captured["sealed"] and not captured["retired"]
        assert captured["allocations"] >= 1
        assert captured["traced_bytes"] >= working.nbytes
        assert captured["backing_bytes"] >= working.nbytes
        assert tuple(captured["pool"]) == pool and captured["device"] == args.device
        working_pointer = working.data_ptr()
        template_pointer = g1.raw_cuda_graph()
        assert template_pointer
        exec_before = None
        if args.mode == "executable":
            g1.instantiate()
            exec_before = g1.raw_cuda_graph_exec()
            assert exec_before
            replay_exact(g1, working, host, stream)

        g2 = torch.cuda.CUDAGraph(keep_graph=True)
        rejected_ref = weakref.ref(g2)
        try:
            with vt.graph(
                g2, pool=pool, stream=stream, reserve_bytes=reserve, footprint=footprint
            ):
                rejected_body_entered = True
        except RuntimeError as error:
            # Validate this natural refusal; do not classify arbitrary failures.
            assert type(error) is RuntimeError and str(error) == REUSED_FOOTPRINT
            refusal = {"type": type(error).__name__, "message": str(error)}
        else:
            raise AssertionError("reusing a live footprint unexpectedly succeeded")
        assert not rejected_body_entered
        after_refusal = same_live_footprint(footprint, captured)
        g2.reset()
        after_reset = same_live_footprint(footprint, captured)
        del g2
        gc.collect()  # No nonblocking collection/timing claim.
        assert rejected_ref() is None
        after_drop = same_live_footprint(footprint, captured)
        assert g1.raw_cuda_graph() == template_pointer

        if args.mode == "template-only":
            # keep_graph=True leaves only the real template through G2's death.
            g1.instantiate()
            exec_before = g1.raw_cuda_graph_exec()
            assert exec_before
            replay_exact(g1, working, host, stream)
        assert g1.raw_cuda_graph_exec() == exec_before
        source.add_(17)
        replay_exact(g1, working, host + 17, stream)
        assert g1.raw_cuda_graph_exec() == exec_before
        assert working.data_ptr() == working_pointer
        after_replay = same_live_footprint(footprint, captured)

        # Drain actual replay before reset. Working remains strongly owned here.
        stream.synchronize()
        before_retire = vt.stats(args.device)
        g1.reset()
        retired = footprint.facts()
        assert retired["retired"] and retired["sealed"]
        assert retired["definition"] == captured["definition"]
        assert working.data_ptr() == working_pointer
        torch.testing.assert_close(working.cpu(), host + 17, rtol=0, atol=0)
        after_retire = vt.stats(args.device)

    return {
        "passed": True,
        "mode": args.mode,
        "torch": torch.__version__,
        "torch_git": torch.version.git_version,
        "helper_sha256": helper_sha,
        "device": args.device,
        "gpu_uuid": actual_uuid,
        "pool": pool,
        "reserve_bytes": reserve,
        "working_bytes": working.nbytes,
        "working_pointer": working_pointer,
        "g1_template_pointer": template_pointer,
        "g1_exact_executable": exec_before,
        "refusal": refusal,
        "rejected_body_entered": rejected_body_entered,
        "rejected_g2_collected": rejected_ref() is None,
        "footprint": {
            "captured": captured,
            "after_g2_refusal": after_refusal,
            "after_g2_reset": after_reset,
            "after_g2_drop": after_drop,
            "after_g1_replay": after_replay,
            "after_g1_reset": retired,
        },
        "working_stats_before_g1_reset": before_retire,
        "working_stats_after_g1_reset": after_retire,
        "limits": [
            "footprint retirement is metadata, not physical GPU release",
            "Working storage remains owned after G1 reset",
            "no callback-registration failure injection or qualification",
            "no Source/Pin/private stream-retirement coverage",
        ],
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", type=int, default=0)
    parser.add_argument("--gpu-uuid", required=True)
    parser.add_argument(
        "--mode", choices=("executable", "template-only"), default="executable"
    )
    parser.add_argument("--expected-helper-sha256", default=HELPER_SHA256)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    torch.set_num_threads(1)
    torch.set_num_interop_threads(1)
    result = run(args)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))
