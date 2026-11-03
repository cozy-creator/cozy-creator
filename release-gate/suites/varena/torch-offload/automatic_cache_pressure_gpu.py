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
            segment = owner.capture(
                block, (example,), stream=stream, reserve_bytes=reserve
            )
            with entries[index].admission():
                entries[index].publish(segment)
                output = segment(example, stream=stream)
            stream.synchronize()
            torch.testing.assert_close(
                output.cpu(),
                torch.nn.functional.linear(torch.ones(1, 32), *controls[index]),
            )
            segments.append(segment)
        del output
        stream.synchronize()
        before = vt.stats(args.device)

        def after_launch(graph):
            fixture.gate_wait(stream.cuda_stream)

        hook = segments[0]._graph.register_replay_end_hook(after_launch)
        request = None
        try:
            # This is the real lock held by an allocating managed component.
            # Callback entry1 must retire WITHOUT acquiring this shared lock.
            with owner._owner.lock, entries[0].admission():
                output = segments[0](example, stream=stream)
                assert fixture.stream_pending(stream.cuda_stream)
                request = _runtime.request_cache_shrink(args.device, reserve)
                assert request is not None
                poll(
                    deadline,
                    lambda: not entries[1].facts()["has_definition"],
                    "idle entry retirement while another entry/admission/device lock is held",
                )
                assert entries[0].facts()["has_definition"]
                assert not segments[0].facts()["closed"]
                assert fixture.stream_pending(stream.cuda_stream)
                logical = vt.stats(args.device)
                trimmed = vt.trim(
                    args.device
                )  # Real physical observation, outside capture.
                physical = vt.stats(args.device)
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
            model[1], (example,), stream=stream, reserve_bytes=reserve
        )
        with entries[1].admission():
            entries[1].publish(rebuilt)
            fresh = rebuilt(example, stream=stream)
        stream.synchronize()
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
