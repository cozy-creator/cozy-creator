"""Real two-block automatic offload client; operator runs CUDA through ordinary Cozy.

CPU mode exercises the real public eager/ownership path, NOT simulated graphs.
CUDA mode requires actual cold measurement, cached replay and native retirement.
"""

from __future__ import annotations

import argparse
import json
import sys
import time

import torch
from tensorfs.varena import _runtime, api
from tensorfs.varena import torch as vt


def poll(timeout, observe, label):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = observe()
        if value:
            return value
        time.sleep(0.005)
    raise TimeoutError("two-block gate deadline: " + label)


def pointers(gpu, regions, stream):
    # These are real normal checked exports, used only by this metadata probe.
    # Function exit drops ALL local export references outside capture/replay.
    with gpu.use(regions, stream.cuda_stream) as used:
        exports = [
            used.tensor(i, dtype="uint8", shape=(1,)) for i in range(len(regions))
        ]
        used.wait_ready()
        result = [used.ptr(i) for i in range(len(regions))]
        for tensor in exports:
            tensor.record_stream(stream)
    return result


def check_parent_observations(owner, model, value, expected, stream, counts):
    """Real typed SDK gates; no authored execution is retried."""
    receipts = []
    for name, supplied, wanted in (
        ("two-block-caller", value, expected),
        (None, value[:1], expected[:1]),
    ):
        before = list(counts.values())
        old_keys = [
            row["native_key"] for row in owner.graph_facts()["boundaries"].values()
        ]
        parent = vt.phase_enter(stream.device, name, mode="eager")
        try:
            result = model(supplied)
            stream.synchronize()
            torch.testing.assert_close(result.cpu(), wanted, rtol=2e-5, atol=2e-6)
            assert list(counts.values()) == [number + 1 for number in before]
            assert all(
                row["reason"] == "parent_observation_active"
                for row in owner.graph_facts()["boundaries"].values()
            )
        finally:
            row = vt.phase_leave(stream.device, parent, completed=False)
        assert row["observation_id"] == parent and row["key"] == name
        assert old_keys == [
            row["native_key"] for row in owner.graph_facts()["boundaries"].values()
        ]
        if name is not None:
            vt.phase_forget(stream.device, name)
        after = list(counts.values())
        resumed = model(value)  # New request, after caller closed its own token.
        stream.synchronize()
        torch.testing.assert_close(resumed.cpu(), expected, rtol=2e-5, atol=2e-6)
        assert list(counts.values()) == after
        receipts.append(
            {
                "parent_key": name,
                "same_token_returned": True,
                "body_calls_under_parent": [
                    right - left for left, right in zip(before, after)
                ],
                "cached_replay_resumed": True,
            }
        )
    return receipts


def run(args):
    device = torch.device(args.device)
    if device.type == "cuda" and device.index is None:
        device = torch.device("cuda", 0)
    torch.set_float32_matmul_precision("highest")  # Functional FP32 parity gate.
    torch.manual_seed(942)
    model = torch.nn.Sequential(
        *[
            torch.nn.Sequential(
                torch.nn.Linear(32, 32), torch.nn.ReLU(), torch.nn.Linear(32, 32)
            )
            for _ in range(2)
        ]
    ).eval()
    cpu_input = torch.arange(64.0).reshape(2, 32) / 64
    with torch.no_grad():
        expected = model(cpu_input)
        changed_expected = model(cpu_input + 1)
    counts = {id(block): 0 for block in model}
    forward_code = torch.nn.Sequential.forward.__code__

    def profile(frame, event, value):
        if event == "call" and frame.f_code is forward_code:
            identity = id(frame.f_locals.get("self"))
            if identity in counts:
                counts[identity] += 1

    owner = vt.offload(
        model, device=device, graphs="auto"
    )  # Installs BEFORE CUDA allocation.
    quarantine_before = api.retirement_stats()["quarantined"]
    previous = sys.getprofile()
    sys.setprofile(profile)
    try:
        if device.type == "cpu":
            with torch.no_grad():
                first = model(cpu_input)
                second = model(cpu_input + 1)
            torch.testing.assert_close(first, expected, rtol=0, atol=0)
            torch.testing.assert_close(second, changed_expected, rtol=0, atol=0)
            facts = owner.graph_facts()
            assert len(facts["boundaries"]) == 2
            assert all(
                row["reason"] == "cpu" and row["cache"] is None
                for row in facts["boundaries"].values()
            )
            assert list(counts.values()) == [2, 2]
            assert not first.is_inference() and not second.is_inference()
            assert not torch.cuda.is_initialized()
            owner.close()
            return {
                "passed": True,
                "device": "cpu",
                "scope": "actual public controller metadata/eager path; CUDA graphs/native observation/CacheOwner NOT exercised",
                "facts": facts,
                "body_calls": list(counts.values()),
                "cuda_initialized": False,
            }

        stream = torch.cuda.Stream(device=device)
        with torch.no_grad(), torch.cuda.stream(stream):
            value = cpu_input.to(device)
            rng_before = torch.cuda.get_rng_state(device).clone()
            first = model(value)  # Cold: EXACTLY ONE original call per block.
            cold = owner.graph_facts()
            assert list(counts.values()) == [1, 1]
            assert all(
                row["cold_observations"] == 1
                and row["native_peak"] is not None
                and row["capture_pool_bytes"] is not None
                and row["captures"] == row["replays"] == 0
                for row in cold["boundaries"].values()
            ), cold
            value.copy_(cpu_input + 1)
            second = model(value)  # Admit measured pool -> warm/capture -> replay.
            promoted = owner.graph_facts()
            assert list(counts.values()) == [3, 3], (
                counts
            )  # Warm + definition capture only.
            assert all(
                row["captures"] == row["replays"] == 1
                and row["cache"]["has_definition"]
                for row in promoted["boundaries"].values()
            ), promoted
            third = model(value)  # Cached replay: NO Python block body.
            assert list(counts.values()) == [3, 3]
            assert all(
                row["replays"] == 2
                for row in owner.graph_facts()["boundaries"].values()
            )
            assert (
                not first.is_inference()
                and not second.is_inference()
                and not third.is_inference()
            )
            assert not torch._C._is_alias_of(
                first, second
            ) and not torch._C._is_alias_of(second, third)
            stream.synchronize()
            torch.testing.assert_close(first.cpu(), expected, rtol=2e-5, atol=2e-6)
            torch.testing.assert_close(
                second.cpu(), changed_expected, rtol=2e-5, atol=2e-6
            )
            torch.testing.assert_close(
                third.cpu(), changed_expected, rtol=2e-5, atol=2e-6
            )
            states = list(owner._automatic._states.values())
            segments = [state.cache.segment for state in states]
            assert all(
                not row["pins"] and not row["unjoined"]
                for segment in segments
                for row in segment._weight_entries()
            )
            regions = list(
                dict.fromkeys(
                    region
                    for segment in segments
                    for region in segment._definition.regions
                )
            )
            before_ptr = pointers(owner._owner.gpu, regions, stream)
            stream.synchronize()
            owner._owner.gpu.trim()  # Explicit fixture reclaim, never product policy.
            assert all(
                row["mapped_bytes"] == 0
                for segment in segments
                for row in segment._weight_entries()
            )
            # The SAME two automatic definitions remain alive across weight eviction.
            fourth = model(value)
            stream.synchronize()
            after_ptr = pointers(owner._owner.gpu, regions, stream)
            assert before_ptr == after_ptr
            assert [state.cache.segment for state in states] == segments
            assert list(counts.values()) == [3, 3]
            torch.testing.assert_close(
                fourth.cpu(), changed_expected, rtol=2e-5, atol=2e-6
            )

            # Real native callback request while model and another entry admission
            # are held. Idle entry1 must retire WITHOUT acquiring the model lock.
            stream.synchronize()
            before_weight_trim = vt.stats(device)
            owner._owner.gpu.trim()  # Ready weights must not satisfy this request.
            weights_before_pressure = [
                row for segment in segments for row in segment._weight_entries()
            ]
            assert all(row["mapped_bytes"] == 0 for row in weights_before_pressure)
            before_pressure = vt.stats(device)
            assert before_pressure["pools_bytes"] == before_weight_trim["pools_bytes"]
            assert [state.cache.segment for state in states] == segments
            request = None
            try:
                with owner._owner.lock, states[0].cache.admission():
                    request = _runtime.request_cache_shrink(
                        device.index, states[1].peak
                    )
                    assert request is not None
                    poll(
                        args.timeout_s,
                        lambda: states[1].cache.segment is None,
                        "native idle cache retirement",
                    )
                    assert states[0].cache.segment is segments[0]
                    request_facts = poll(
                        args.timeout_s,
                        lambda: (
                            facts if (facts := request.facts())["finished"] else None
                        ),
                        "native request completion",
                    )
                    assert (
                        request_facts["returned"] >= 1 and not request_facts["failed"]
                    )
                    idle_after_request = states[1].cache.facts()
                    assert idle_after_request["native_retirements"] >= 1
                    stream.synchronize()  # Join the actual completed release tail.
                    vt.trim(device)
                    physical = vt.stats(device)
                    assert physical["pools_bytes"] < before_pressure["pools_bytes"]
            finally:
                if request is not None:
                    request.cancel()
            fifth = model(value)  # Rebuild only the naturally retired entry.
            stream.synchronize()
            torch.testing.assert_close(
                fifth.cpu(), changed_expected, rtol=2e-5, atol=2e-6
            )
            assert list(counts.values()) == [3, 5]
            parent_receipts = check_parent_observations(
                owner,
                model,
                value,
                changed_expected,
                stream,
                counts,
            )
            final = owner.graph_facts()
            assert [row["captures"] for row in final["boundaries"].values()] == [1, 2]
            assert not owner._intent_frames and not owner._intent_failed
            assert torch.equal(rng_before, torch.cuda.get_rng_state(device))
        owner.close()  # Drains native callbacks outside model/entry locks.
        assert api.retirement_stats()["quarantined"] == quarantine_before
        return {
            "passed": True,
            "device": str(device),
            "torch": torch.__version__,
            "core": api.core_identity(),
            "scope": "actual nn-only automatic cold/admission/capture/replay/native idle-pressure retirement",
            "cold": cold,
            "promoted": promoted,
            "final": final,
            "same_va_after_weight_refill": before_ptr == after_ptr,
            "all_definitions_live_with_zero_weight_pins": True,
            "independent_normal_outputs": True,
            "body_calls": list(counts.values()),
            "weights_before_pressure": weights_before_pressure,
            "pool_bytes_before_weight_trim": before_weight_trim["pools_bytes"],
            "pool_bytes_before_pressure": before_pressure["pools_bytes"],
            "pool_bytes_after_pressure": physical["pools_bytes"],
            "idle_cache_after_request": idle_after_request,
            "native_request": request_facts,
            "parent_observation_control": "typed",
            "parent_observation_receipts": parent_receipts,
            "typed_parent_observation_eager_choice": True,
            "matmul_precision": torch.get_float32_matmul_precision(),
            "rng_state_preserved": True,
            "unkeyed_parent_preserves_old_key_across_shape_change": True,
        }
    finally:
        sys.setprofile(previous)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", default="cuda:0")
    parser.add_argument("--timeout-s", type=float, default=30)
    options = parser.parse_args()
    torch.set_num_threads(1)
    torch.set_num_interop_threads(1)
    print(json.dumps(run(options), indent=2))
