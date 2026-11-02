"""Prepared coordinator client for real native Use readiness; not GPU-qualified.

Run one mode per owned child with the matched extension/core/provider and an
explicit whole-child coordinator deadline. No rentals, models or fake callbacks.
"""

from __future__ import annotations

import argparse
import gc
import json
import math
import os
import threading
import time

import torch
from provider import Provider
from tensorfs.varena import _runtime, api
from tensorfs.varena import torch as vt

MODES = (
    "retirement_pending",
    "unrelated_retirement",
    "capture_refusal",
    "concurrent_close",
)
NBYTES = 1 << 20


def poll(deadline, observe, label):
    while time.monotonic() < deadline:
        value = observe()
        if value:
            return value
        time.sleep(0.005)
    raise TimeoutError(f"explicit client deadline expired waiting for {label}")


def spawn(call, name):
    started, done, result = threading.Event(), threading.Event(), {}

    def worker():
        started.set()  # Python scheduling only; this cannot prove C ABI entry.
        try:
            result["value"] = call()
        except Exception as error:  # noqa: BLE001 - preserve native thread failure for joined().
            result["error"] = error
        finally:
            done.set()

    thread = threading.Thread(target=worker, name=name, daemon=True)
    thread.start()
    return thread, started, done, result


def joined(task, deadline):
    thread, _, done, result = task
    thread.join(max(0, deadline - time.monotonic()))
    if thread.is_alive():
        raise TimeoutError(
            f"explicit client deadline expired waiting for {thread.name}"
        )
    assert done.is_set()
    return result


def success(result):
    if "error" in result:
        raise result["error"]
    assert result["value"] is None, result


def busy(fixture):
    try:
        fixture.close()
    except api.Busy:
        return
    raise AssertionError("helper released real native owners/held mutexes")


def copied(used, stream, output):
    weight = used.tensor(dtype="uint8", shape=(NBYTES,))
    with torch.cuda.stream(stream):
        output.copy_(weight)
    return weight


def retirement_case(gpu, fixture, streams, output, deadline, concurrent):
    selected, _, unrelated = streams
    fixture.job_retirement_hold(True)
    fixture.extra_gate_wait(unrelated.cuda_stream)
    used = gpu.use([fixture.region], selected.cuda_stream)
    pointer = used.ptr()
    weight = copied(used, selected, output)
    assert fixture.source_pending() and fixture.stream_pending(selected.cuda_stream)
    assert fixture.stream_pending(unrelated.cuda_stream)
    waiter = spawn(used.wait_ready, "use-ready-wait")
    poll(deadline, waiter[1].is_set, "Python readiness caller scheduling")
    fixture.gate_set(1)
    selected.synchronize()  # Actual selected producer/consumer completion.
    pending = poll(deadline, fixture.job_retirement_pending, "actual Job VR_BUSY")
    observed = fixture.counts()
    assert observed["prepares"] == observed["submits"] == 1, observed
    assert observed["job_retired"] == observed["job_dropped"] == 0, observed
    assert not waiter[2].is_set(), waiter[3]
    # New Use after publication: its Ready Pin has no original submission Pin.
    # It must still observe this same Control's unfinished resource retirement.
    second = gpu.use([fixture.region], streams[1].cuda_stream)
    assert second._handle != used._handle and second.ptr() == pointer
    second_waiter = spawn(second.wait_ready, "use-ready-published-wait")
    poll(deadline, second_waiter[1].is_set, "Python published Use caller scheduling")
    assert not second_waiter[2].is_set(), second_waiter[3]
    after_second = fixture.counts()
    assert after_second["prepares"] == after_second["submits"] == 1, after_second
    assert torch.equal(output.cpu(), torch.full((NBYTES,), 73, dtype=torch.uint8))
    del weight
    gc.collect()
    busy(fixture)
    closer = None
    if concurrent:
        fixture.region.close()
        closer = spawn(used.close, "use-ready-close")
        poll(deadline, closer[1].is_set, "Python concurrent close scheduling")
    fixture.job_retirement_hold(False)
    result = joined(waiter, deadline)
    if concurrent and isinstance(result.get("error"), api.Invalid):
        wait_outcome = "close won before native wait acquired Use"
    else:
        success(result)
        wait_outcome = "ready"
    success(joined(second_waiter, deadline))
    second.close()
    if closer:
        success(joined(closer, deadline))
    else:
        used.close()
        fixture.region.close()
    assert fixture.stream_pending(unrelated.cuda_stream)
    return {
        "producer_pending_observed": True,
        "selected_stream_completed": True,
        "job_busy_returns_before_release": pending,
        "counts_while_job_held": observed,
        "published_use_same_pointer": True,
        "counts_after_second_use": after_second,
        "published_use_ready_after_retirement_release": True,
        "distinct_uses": True,
        "unrelated_stream_pending_after_wait": True,
        "wait_outcome": wait_outcome,
        "concurrent_close_stress": concurrent,
        "native_wait_entry_proven": False,
    }


def unrelated_case(gpu, fixture, streams, output, deadline):
    first, selected, unrelated = streams
    fixture.extra_gate_wait(unrelated.cuda_stream)
    used = gpu.use([fixture.region], first.cuda_stream)
    assert fixture.source_pending()
    fixture.gate_set(1)
    used.wait_ready()
    assert fixture.counts()["job_dropped"] == 1
    fixture.source_retirement_hold(True)
    used.close()
    fixture.region.close()
    pending = poll(
        deadline, lambda: fixture.counts()["source_pending"], "actual Source VR_BUSY"
    )
    before = api.retirement_stats()
    assert before["queued"] > 0 and fixture.counts()["source_dropped"] == 0
    busy(fixture)
    region = gpu.region(bytes([31]) * NBYTES)
    selected_use = gpu.use([region], selected.cuda_stream)
    selected_use.wait_ready()  # Must not drain the unrelated retained Source.
    assert fixture.counts()["source_dropped"] == 0
    assert api.retirement_stats()["queued"] > 0
    assert fixture.stream_pending(unrelated.cuda_stream)
    weight = copied(selected_use, selected, output)
    selected.synchronize()
    assert torch.equal(output.cpu(), torch.full((NBYTES,), 31, dtype=torch.uint8))
    del weight
    selected_use.close()
    region.close()
    fixture.source_retirement_hold(False)
    return {
        "source_busy_returns_before_selected_wait": pending,
        "retirement_before_selected_wait": before,
        "selected_ready_while_unrelated_source_retirement_pending": True,
        "unrelated_stream_pending_after_wait": True,
        "exact_readback": 31,
    }


def capture_case(gpu, fixture, streams, scratch):
    first, capture, _ = streams
    used = gpu.use([fixture.region], first.cuda_stream)
    assert fixture.source_pending()
    fixture.gate_set(1)
    used.wait_ready()
    handle, pointer = used._handle, used.ptr()

    def refused():
        try:
            used.wait_ready()
        except api.Capturing:
            assert used._handle == handle
            return
        raise AssertionError("wait_ready admitted actual capture/replay")

    graph = torch.cuda.CUDAGraph()
    with vt.graph(graph, stream=capture, reserve_bytes=0):
        scratch.add_(1)
        refused()
    capture.synchronize()
    with _runtime.replay(gpu.device):
        vt.replay(graph, stream=capture)
        refused()
    capture.synchronize()
    used.wait_ready()
    assert used._handle == handle and used.ptr() == pointer
    graph.reset()
    used.close()
    fixture.region.close()
    return {
        "actual_capture_refused": True,
        "actual_replay_guard_refused": True,
        "same_use_and_pointer_after_refusal": True,
    }


def uuid(value):
    return str(value).lower().removeprefix("gpu-").replace("-", "")


def run(args):
    # Resolve the production ABI/method and all helper capabilities before CUDA.
    if not callable(getattr(api.Use, "wait_ready", None)) or not callable(
        api._lib.vr_v1_use_wait_ready
    ):
        raise TypeError("matching native Use.wait_ready capability is required")
    provider = Provider(args.provider)
    deadline = time.monotonic() + args.timeout_s
    vt.install(args.device)
    torch.cuda.set_device(args.device)
    actual_uuid = str(torch.cuda.get_device_properties(args.device).uuid)
    assert len(uuid(args.gpu_uuid)) == 32 and uuid(actual_uuid) == uuid(args.gpu_uuid)
    streams = tuple(torch.cuda.Stream(device=args.device) for _ in range(3))
    output = torch.empty(NBYTES, dtype=torch.uint8, device="cuda")
    scratch = torch.zeros(1, dtype=torch.uint8, device="cuda")
    torch.cuda.current_stream().synchronize()
    with torch.cuda.stream(streams[0]):
        output.fill_(0)
    with torch.cuda.stream(streams[1]):
        scratch.add_(1)
    for stream in streams:
        stream.synchronize()
    gpu = api.open(args.device)
    try:
        api._call("use_wait_ready", gpu._handle)
    except api.Invalid:
        pass
    else:
        raise AssertionError("readiness accepted a real handle of the wrong type")
    initial = gpu.stats()
    quarantine = api.retirement_stats()["quarantined"]
    fixture = provider.create(gpu, NBYTES)
    if args.mode in ("retirement_pending", "concurrent_close"):
        observations = retirement_case(
            gpu, fixture, streams, output, deadline, args.mode == "concurrent_close"
        )
    elif args.mode == "unrelated_retirement":
        observations = unrelated_case(gpu, fixture, streams, output, deadline)
    else:
        observations = capture_case(gpu, fixture, streams, scratch)
    fixture.gate_set(1)
    fixture.extra_gate_set(1)
    for stream in streams:
        stream.synchronize()
    poll(
        deadline,
        lambda: fixture.counts()["source_dropped"] == 1,
        "checked native Source retirement",
    )
    counts = fixture.counts()
    assert counts["prepares"] == counts["submits"] == 1, counts
    assert counts["job_retired"] == counts["job_dropped"] == 1, counts
    assert counts["source_retired"] == counts["source_dropped"] == 1, counts
    fixture.close()
    del output, scratch
    gc.collect()
    gpu.trim()
    final = gpu.stats()
    assert final["weights_resident"] == initial["weights_resident"], (initial, final)
    for stream in streams:
        vt.retire_stream(stream, args.device)
    gpu.close()
    poll(
        deadline,
        lambda: not api.retirement_stats()["queued"],
        "final checked retirement",
    )
    assert api.retirement_stats()["quarantined"] == quarantine
    return {
        "passed": True,
        "mode": args.mode,
        "gpu_uuid": actual_uuid,
        "device": args.device,
        "pid": os.getpid(),
        "torch": str(torch.__version__),
        "observations": observations,
        "counts": counts,
        "initial_native_stats": initial,
        "final_native_stats": final,
        "retirement": api.retirement_stats(),
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", required=True)
    parser.add_argument("--device", type=int, default=0)
    parser.add_argument("--gpu-uuid", required=True)
    parser.add_argument("--mode", choices=MODES, required=True)
    parser.add_argument("--timeout-s", type=float, default=60)
    args = parser.parse_args()
    if not math.isfinite(args.timeout_s) or args.timeout_s <= 0:
        parser.error("--timeout-s must be positive and finite")
    print(json.dumps(run(args)))
