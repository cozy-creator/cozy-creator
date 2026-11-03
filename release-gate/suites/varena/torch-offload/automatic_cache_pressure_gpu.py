"""Prepared real graph-cache pressure while another entry owns live execution.

Coordinator-only isolated ordinary-Cozy child, with the existing matched C gate
provider. No simulated graph, pool, callback selection or physical byte credit.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import time
from pathlib import Path

import torch
from tensorfs.varena import _runtime, api
from tensorfs.varena import torch as vt
from tensorfs.varena._offload_auto import _GraphCacheEntry
from tensorfs.varena._offload_graph import _UNPROVEN


def poll(deadline, observe, label):
    while time.monotonic() < deadline:
        result = observe()
        if result:
            return result
        time.sleep(0.005)
    raise TimeoutError("explicit graph cache fixture deadline: " + label)


def run(args):
    deadline = time.monotonic() + args.timeout_s
    spec = importlib.util.spec_from_file_location(
        "cache_real_gate_provider", args.provider_python
    )
    provider_module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(provider_module)
    torch.manual_seed(29)
    model = torch.nn.Sequential(torch.nn.Linear(32, 32), torch.nn.Linear(32, 32)).eval()
    controls = [
        (block.weight.detach().clone(), block.bias.detach().clone()) for block in model
    ]
    owner = vt.offload(model, device=args.device)
    gpu = owner._owner.gpu
    stream = torch.cuda.Stream(device=args.device)
    idle_stream = torch.cuda.Stream(device=args.device)
    assert stream.cuda_stream != idle_stream.cuda_stream
    with torch.cuda.device(args.device):
        provider = provider_module.Provider(args.provider)
        fixture = provider.create(gpu, 32, mode="ordering")
    quarantine = api.retirement_stats()["quarantined"]
    entries = [_GraphCacheEntry(args.device) for _ in model]
    reserve = 2 << 20  # Explicit test fixture target, not automatic product sizing.
    segments = []
    with torch.inference_mode(), torch.cuda.stream(stream):
        example = torch.ones(1, 32, device=args.device)
        for index, block in enumerate(model):
            capture_stream = stream if index == 0 else idle_stream
            segment = owner.capture(
                block, (example,), stream=capture_stream, reserve_bytes=reserve
            )
            with entries[index].admission():
                entries[index].publish(segment)
                output = segment(example, stream=capture_stream)
            capture_stream.synchronize()
            torch.testing.assert_close(
                output.cpu(),
                torch.nn.functional.linear(torch.ones(1, 32), *controls[index]),
            )
            segments.append(segment)
        del output
        stream.synchronize()
        idle_stream.synchronize()
        before = vt.stats(args.device)
        # A shrink request may finish from ready weight/Working backing without
        # offering a graph owner. Remove that setup supply before this callback
        # fixture; both live definitions and their private pools remain owned.
        ready_before = gpu.stats()
        ready_trimmed = gpu.trim()
        ready_after = gpu.stats()
        ready_working_after = vt.stats(args.device)
        assert ready_after["weights_resident"] == 0
        assert ready_working_after["pools_bytes"] == before["pools_bytes"]
        ready_precondition = {
            "trimmed_bytes": ready_trimmed,
            "gpu_before": ready_before,
            "gpu_after": ready_after,
            "working_after": ready_working_after,
        }

        def after_launch(graph):
            fixture.gate_wait(stream.cuda_stream)

        hook = segments[0]._graph.register_replay_end_hook(after_launch)
        request = None

        def diagnostic(label):
            # Diagnostic successor only: passive reads, no callback or recovery call.
            def read(operation):
                try:
                    return operation()
                except Exception as error:
                    return {"observation_error": type(error).__name__, "message": str(error)}

            def segment_state(segment):
                held = segment._lock.acquire(blocking=False)
                if not held:
                    return {"lock_acquired": False}
                try:
                    return {
                        "lock_acquired": True,
                        "closed": segment._closed,
                        "closing": segment._closing,
                        "busy": segment._busy,
                        "has_failure": segment._failure is not None,
                        "tail_present": segment._tail is not None,
                        "tail_ready": None if segment._tail is None else read(segment._tail.query),
                    }
                finally:
                    segment._lock.release()

            print(json.dumps({
                "diagnostic": "cache-pressure-passive-v1",
                "stage": label,
                "elapsed_s": time.monotonic() - (deadline - args.timeout_s),
                "requested_bytes": reserve,
                "ready_backing_precondition": ready_precondition,
                "request": None if request is None else read(request.facts),
                "entries": [read(entry.facts) for entry in entries],
                "segments": [read(lambda segment=segment: segment_state(segment)) for segment in segments],
                "working": read(lambda: vt.stats(args.device)),
                "gpu": read(gpu.stats),
                "stream_pending": read(lambda: fixture.stream_pending(stream.cuda_stream)),
                "idle_stream_pending": read(lambda: fixture.stream_pending(idle_stream.cuda_stream)),
                "stream_ids": {"active": stream.cuda_stream, "idle": idle_stream.cuda_stream},
                "unproven_count": len(_UNPROVEN),
                "observation_note": "Sequential passive observations; entry0 RLock is reentrant for the sampling caller. No callback/reclaim/trim is invoked by diagnostics.",
            }), flush=True)

        diagnostic("before_active_replay")
        try:
            # This is the real lock held by an allocating managed component.
            # Callback entry1 must retire WITHOUT acquiring this shared lock.
            with owner._owner.lock, entries[0].admission():
                output = segments[0](example, stream=stream)
                assert fixture.stream_pending(stream.cuda_stream)
                diagnostic("held_before_request")
                request = _runtime.request_cache_shrink(args.device, reserve)
                assert request is not None
                diagnostic("held_after_request")
                try:
                    poll(
                        deadline,
                        lambda: not entries[1].facts()["has_definition"],
                        "idle entry retirement while another entry/admission/device lock is held",
                    )
                except BaseException:
                    diagnostic("held_idle_poll_failure")
                    raise
                diagnostic("held_idle_retired")
                assert entries[0].facts()["has_definition"]
                assert not segments[0].facts()["closed"]
                assert fixture.stream_pending(stream.cuda_stream)
                logical = vt.stats(args.device)
                trimmed = vt.trim(
                    args.device
                )  # Real physical observation, outside capture.
                physical = vt.stats(args.device)
                diagnostic("held_after_physical_trim")
                assert before["pools_bytes"] - physical["pools_bytes"] >= reserve
                assert physical["pools_bytes"] >= reserve
                assert fixture.stream_pending(stream.cuda_stream)
                request_facts = poll(
                    deadline,
                    lambda: facts if (facts := request.facts())["finished"] else None,
                    "actual native shrink request completion",
                )
                assert request_facts["returned"] >= 1 and not request_facts["failed"]
        finally:
            fixture.gate_set(1)
            hook.remove()
            if request is not None:
                request.cancel()
        stream.synchronize()
        torch.testing.assert_close(
            output.cpu(), torch.nn.functional.linear(torch.ones(1, 32), *controls[0])
        )
        assert (
            segments[1].facts()["closed"] and not entries[1].facts()["has_definition"]
        )
        # Ordinary actual rebuild is separate from the pressure request; no
        # authored computation is retried after an exception.
        rebuilt = owner.capture(
            model[1], (example,), stream=idle_stream, reserve_bytes=reserve
        )
        with entries[1].admission():
            entries[1].publish(rebuilt)
            fresh = rebuilt(example, stream=idle_stream)
        idle_stream.synchronize()
        torch.testing.assert_close(
            fresh.cpu(), torch.nn.functional.linear(torch.ones(1, 32), *controls[1])
        )
    # Native registrations drain outside device/entry locks. Entries stop
    # admission and preserve exact retryable ownership on any close refusal.
    for entry in entries:
        entry.close()
    owner.close()
    fixture.region.close()
    poll(
        deadline,
        lambda: fixture.counts()["source_dropped"] == 1,
        "own unused Source retirement",
    )
    fixture.close()
    gpu.close()
    assert not _UNPROVEN
    assert api.retirement_stats()["quarantined"] == quarantine
    print(
        json.dumps(
            {
                "gpu_executed": True,
                "torch": torch.__version__,
                "idle_callback_progress_with_active_entry_and_device_lock": True,
                "active_gated_graph_remained_owned": True,
                "before": before,
                "after_logical_retirement": logical,
                "after_physical_trim": physical,
                "explicit_trimmed_bytes": trimmed,
                "native_request_facts": request_facts,
                "idle_entry_rebuilt_with_exact_values": True,
                "full_automatic_dispatch_or_unknown_failure_cleanup": "not exercised",
            }
        )
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--device", type=int, default=0)
    parser.add_argument("--provider", type=Path, required=True)
    parser.add_argument("--provider-python", type=Path, required=True)
    parser.add_argument("--timeout-s", type=float, default=60)
    run(parser.parse_args())
