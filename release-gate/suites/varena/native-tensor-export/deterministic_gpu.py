"""Coordinator-only CUDA proofs using the existing real C Source provider.

Every invocation admits exactly one Source in one isolated child. Mapped host
gates establish actual pending work; no timed CUDA delay stands in for that fact.
The coordinator owns the whole-process deadline and child cleanup. This client
downloads no model, rents no GPU and makes no claim until its real GPU run passes.
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
from tensorfs import _ext
from tensorfs.varena import api
from tensorfs.varena import torch as vt

MODES = ("ordering", "partial", "prepare_error", "capture_refusal", "failed_tail")
NBYTES = 1 << 20
LAYOUT = {"dtype": "uint8", "shape": (4, 7), "strides": (19, 2), "byte_offset": 17}
INTERIOR = {
    "dtype": "uint8",
    "shape": (3, 3),
    "strides": (4, 19),
    "byte_offset": 38,
}


def uuid_text(value: str) -> str:
    result = str(value).lower().removeprefix("gpu-").replace("-", "")
    if len(result) != 32 or any(char not in "0123456789abcdef" for char in result):
        raise ValueError(f"expected an actual GPU UUID, got {value!r}")
    return result


def poll(deadline, observe, label):
    """Wait for checked facts within the explicit client protocol deadline."""
    while True:
        if time.monotonic() >= deadline:
            raise TimeoutError(f"explicit client deadline expired waiting for {label}")
        value = observe()
        if value:
            return value
        time.sleep(0.005)


def busy(fixture):
    try:
        fixture.close()
    except api.Busy:
        return
    raise AssertionError(
        "helper released gates/code before checked native owners ended"
    )


def counts(fixture, *, prepares=1, submits=1, jobs=0, sources=0):
    value = fixture.counts()
    expected = {
        "prepares": prepares,
        "submits": submits,
        "job_retired": jobs,
        "job_dropped": jobs,
        "source_retired": sources,
        "source_dropped": sources,
    }
    for key, expected_value in expected.items():
        assert value[key] == expected_value, (key, expected, value)
    return value


def settled(fixture, deadline, quarantine, *, submits=1, sources=1):
    def observe():
        value = fixture.counts()
        retirement = api.retirement_stats()
        assert retirement["quarantined"] == quarantine, retirement
        assert value["prepares"] == 1 and value["submits"] == submits, value
        assert value["job_dropped"] <= 1 and value["source_dropped"] <= sources, value
        return (
            value["job_dropped"] == 1
            and value["source_dropped"] == sources
            and not retirement["queued"]
        )

    poll(deadline, observe, "checked native Source/Job retirement")
    return counts(fixture, submits=submits, jobs=1, sources=sources)


def warm(device, streams):
    """Allocate all outputs and warm every copy/capture kernel before any gate."""
    first, second, side, _, capture = streams
    cuda = torch.device("cuda", device)
    with torch.cuda.stream(first):
        source = torch.empty_strided((4, 7), (19, 2), dtype=torch.uint8, device=cuda)
        source.fill_(73)
        outputs = {
            "first": torch.empty((4, 7), dtype=torch.uint8, device=cuda),
            "override": torch.empty((4, 7), dtype=torch.uint8, device=cuda),
            "escaped": torch.empty((3, 3), dtype=torch.uint8, device=cuda),
            "independent": torch.empty((7, 4), dtype=torch.uint8, device=cuda),
            "interior": torch.empty((3, 3), dtype=torch.uint8, device=cuda),
        }
        outputs["first"].copy_(source)
    with torch.cuda.stream(second):
        second.wait_stream(first)
        outputs["override"].copy_(source)
    with torch.cuda.stream(side):
        side.wait_stream(first)
        interior = source[1:, 1::2].detach().T
        outputs["escaped"].copy_(interior)
        outputs["interior"].copy_(interior)
        outputs["independent"].copy_(source.T)
    with torch.cuda.stream(capture):
        scratch = torch.zeros(1, dtype=torch.uint8, device=cuda)
        scratch.add_(1)
    for stream in streams:
        stream.synchronize()
    del source, interior
    expected = {
        name: torch.full(tuple(output.shape), 73, dtype=torch.uint8, device="cpu")
        for name, output in outputs.items()
    }
    return outputs, scratch, expected


def same_values(output, expected):
    assert torch.equal(output.cpu(), expected)


def trim_baseline(gpu, fixture, unrelated, baseline):
    assert fixture.stream_pending(unrelated.cuda_stream)
    gpu.trim()
    assert fixture.stream_pending(unrelated.cuda_stream), (
        "unrelated gated work completed during trim"
    )
    after = gpu.stats()
    assert after["weights_resident"] == baseline["weights_resident"], (baseline, after)
    assert after["working_live"] == baseline["working_live"], (baseline, after)
    return after


def ordering(gpu, fixture, streams, outputs, expected, baseline, deadline, quarantine):
    first, second, side, unrelated, _ = streams
    checks, observations = [], {}
    region = fixture.region
    used = gpu.use([region], first.cuda_stream)
    base = used.ptr()
    first_export = used.tensor(**LAYOUT)
    second_export = used.tensor(**LAYOUT, stream=second.cuda_stream)
    interior_export = used.tensor(**INTERIOR, stream=second.cuda_stream)
    assert first_export.data_ptr() == second_export.data_ptr() == base + 17
    assert interior_export.data_ptr() == base + 38
    assert first_export.stride() == LAYOUT["strides"]
    assert interior_export.stride() == INTERIOR["strides"]
    with torch.cuda.stream(first):
        outputs["first"].copy_(first_export)
    with torch.cuda.stream(second):
        # There is deliberately no first-stream dependency on the override path.
        outputs["override"].copy_(second_export)
    assert fixture.source_pending()
    assert fixture.stream_pending(first.cuda_stream)
    assert fixture.stream_pending(second.cuda_stream)
    observations["counts_while_producer_pending"] = counts(fixture)
    observations["actual_producer_and_both_consumers_pending"] = True
    busy(fixture)
    checks.append(
        "actual pending producer orders typed first and independent override streams"
    )

    escaped = first_export[1:, 1::2].detach().T
    independent = second_export.detach().T
    interior = interior_export.detach()
    assert escaped.data_ptr() == interior.data_ptr() == base + 38
    assert escaped.stride() == interior.stride() == INTERIOR["strides"]
    used.close()
    region.close()
    del used, region, first_export, second_export, interior_export
    gc.collect()
    assert fixture.source_pending()
    counts(fixture)
    pinned = gpu.stats()["weights_resident"]
    assert pinned > baseline["weights_resident"]
    gpu.trim()
    assert gpu.stats()["weights_resident"] == pinned
    assert fixture.source_pending()
    checks.append(
        "early Use/Region close retains escaped storage Pins and Source ownership"
    )

    fixture.gate_set(1)
    first.synchronize()
    second.synchronize()
    settled(fixture, deadline, quarantine, sources=0)
    same_values(outputs["first"], expected["first"])
    same_values(outputs["override"], expected["override"])
    observations["initial_typed_readback_exact_73"] = True

    # The producer is checked retired before reusing its HOST gate for a later
    # consumer. Gate1 belongs only to the independent unrelated sentinel stream.
    fixture.gate_set(0)
    fixture.extra_gate_wait(unrelated.cuda_stream)
    fixture.gate_wait(side.cuda_stream)
    with torch.cuda.stream(side):
        escaped.record_stream(side)
        independent.record_stream(side)
        interior.record_stream(side)
        outputs["escaped"].copy_(escaped)
        outputs["independent"].copy_(independent)
        outputs["interior"].copy_(interior)
    assert fixture.stream_pending(side.cuda_stream)
    assert fixture.stream_pending(unrelated.cuda_stream)
    counts(fixture, jobs=1)
    # Use.close recorded first. Releasing the independent second-stream storage
    # families first adds second then side; releasing escaped last updates those
    # same events. Native fence insertion order is therefore first, second, side.
    del independent
    del interior
    del escaped
    gc.collect()
    assert fixture.stream_pending(side.cuda_stream)
    observations["later_side_consumer_pending_at_final_alias_release"] = True

    # The existing driver counters increment before cuEventSynchronize. Their
    # same-cohort observer reads atomics without CUDA or the Arena state lock,
    # so it remains usable while trim owns that lock and waits for side's fence.
    event_baseline = _ext._varena_public_gpu_event_counts(gpu._handle)
    trim_done = threading.Event()
    trim_result = {}

    def trim_worker():
        try:
            # api.py's CDLL call releases the GIL while native trim waits.
            trim_result["released"] = gpu.trim()
        except BaseException as error:  # noqa: BLE001 - propagate thread failures to the proof owner.
            trim_result["error"] = error
        finally:
            trim_done.set()

    worker = threading.Thread(target=trim_worker, name="typed-export-trim", daemon=True)
    worker.start()

    def entered_side_wait():
        current = _ext._varena_public_gpu_event_counts(gpu._handle)
        if trim_done.is_set():
            raise AssertionError(
                "trim ended with both gates closed; missing later consumer fence",
                event_baseline,
                current,
                trim_result,
            )
        return current if current[2] - event_baseline[2] >= 3 else None

    wait_entries = poll(
        deadline, entered_side_wait, "trim entering the third native fence wait"
    )
    assert fixture.stream_pending(side.cuda_stream)
    assert fixture.stream_pending(unrelated.cuda_stream)
    assert not trim_done.is_set()
    observations["native_event_counts_before_trim"] = event_baseline
    observations["native_event_counts_with_both_gates_closed"] = wait_entries
    observations["trim_entered_third_fence_wait_before_consumer_gate_open"] = True

    # Counter entry identifies the selected side fence; it is not a completion
    # certificate. Actual stream completion, exact readback and native physical
    # retirement are checked independently after opening only this consumer gate.
    fixture.gate_set(1)
    worker.join(max(0, deadline - time.monotonic()))
    if worker.is_alive():
        raise TimeoutError(
            "explicit client deadline expired waiting for trim to finish"
        )
    assert trim_done.is_set()
    if "error" in trim_result:
        raise trim_result["error"]
    observations["trim_released_bytes"] = trim_result["released"]
    assert fixture.stream_pending(unrelated.cuda_stream)
    side.synchronize()
    observations["physical_baseline_with_unrelated_gate_pending"] = trim_baseline(
        gpu, fixture, unrelated, baseline
    )
    observations["final_counts"] = settled(fixture, deadline, quarantine)
    assert fixture.stream_pending(unrelated.cuda_stream)
    checks.append(
        "trim selects the later interior consumer fence with both gates closed; unrelated work stays pending"
    )
    fixture.extra_gate_set(1)
    unrelated.synchronize()
    for name in ("escaped", "independent", "interior"):
        same_values(outputs[name], expected[name])
    fixture.close()
    checks.append(
        "later strided/detached/transposed/interior reads equal exact Source byte73"
    )
    return checks, observations


def failure(gpu, fixture, streams, baseline, deadline, quarantine, mode):
    first, _, _, unrelated, _ = streams
    submits = int(mode == "partial")
    # Any context-wide fallback in prepare/abort/retirement would also wait for
    # this independent gate. The parent owns terminating a deadline failure.
    fixture.extra_gate_wait(unrelated.cuda_stream)
    used = None
    try:
        used = gpu.use([fixture.region], first.cuda_stream)
    except api.SourceError:
        pass
    else:
        raise AssertionError(f"{mode} Source produced a public Use/export")
    assert used is None
    assert fixture.source_pending() == bool(submits)
    before = counts(fixture, submits=submits)
    assert fixture.stream_pending(unrelated.cuda_stream)
    busy(fixture)
    if mode == "partial":
        # A second read is a negative admission check, not an application retry.
        # The closed failed producer is still gated: contents remain Loading,
        # and the core must refuse without invoking the Source a second time.
        try:
            forbidden = gpu.use([fixture.region], first.cuda_stream)
        except api.Busy:
            pass
        else:
            forbidden.close()
            raise AssertionError("partial failed Source exposed pending contents")
        counts(fixture, submits=1)
        assert fixture.source_pending()
    fixture.region.close()
    if mode == "partial":
        assert fixture.source_pending()
        counts(fixture, submits=1)
        busy(fixture)
    fixture.gate_set(1)
    final = settled(fixture, deadline, quarantine, submits=submits)
    after = trim_baseline(gpu, fixture, unrelated, baseline)
    fixture.extra_gate_set(1)
    unrelated.synchronize()
    fixture.close()
    return [
        f"{mode} real C Source error returns no public Use or tensor",
        "checked native failure owners survive until retirement; no application retry",
        "failed Source reaches physical/source baseline while unrelated gate remains pending",
    ], {"failure_counts": before, "final_counts": final, "physical_baseline": after}


def capture_case(
    gpu,
    fixture,
    streams,
    outputs,
    scratch,
    expected,
    baseline,
    deadline,
    quarantine,
    mode,
):
    first, _, _, unrelated, capture = streams
    checks, observations = [], {}
    used = gpu.use([fixture.region], first.cuda_stream)
    weight = used.tensor(**LAYOUT)
    with torch.cuda.stream(first):
        outputs["first"].copy_(weight)
    assert fixture.source_pending() and fixture.stream_pending(first.cuda_stream)
    counts(fixture)
    fixture.gate_set(1)
    first.synchronize()
    settled(fixture, deadline, quarantine, sources=0)
    same_values(outputs["first"], expected["first"])
    resident = gpu.stats()["weights_resident"]
    assert resident > baseline["weights_resident"]
    if mode == "failed_tail":
        used.close()
        fixture.region.close()
        del used
        poll(
            deadline,
            lambda: not api.retirement_stats()["queued"],
            "closed public handles",
        )

    graph = torch.cuda.CUDAGraph()
    # vt.graph performs a device synchronize during PREPARATION. Both mapped
    # gates are idle here; the unrelated sentinel is armed only after capture.
    with vt.graph(graph, stream=capture, reserve_bytes=0):
        scratch.add_(1)
        calls = vt.stats(gpu.device)["driver_memory_calls"]
        if mode == "capture_refusal":
            try:
                unwanted = used.tensor(**LAYOUT, stream=capture.cuda_stream)
            except RuntimeError as error:
                assert "during CUDA graph capture" in str(error), str(error)
            else:
                del unwanted
                raise AssertionError("typed export was admitted during actual capture")
            assert api.retirement_stats()["quarantined"] == quarantine
        else:
            del weight  # Its only foreign storage owner ends inside actual capture.
            assert api.retirement_stats()["quarantined"] == quarantine + 1
            assert gpu.stats()["weights_resident"] == resident
        assert vt.stats(gpu.device)["driver_memory_calls"] == calls
    capture.synchronize()
    graph.reset()
    del graph
    capture.synchronize()

    if mode == "capture_refusal":
        with torch.cuda.stream(first):
            outputs["override"].copy_(weight)
        first.synchronize()
        same_values(outputs["override"], expected["override"])
        used.close()
        fixture.region.close()
        del used, weight
        gc.collect()
        checks.append(
            "typed export refuses actual capture; original storage remains valid afterward"
        )

    # No capture preparation can now perform its mandatory synchronize behind
    # this independent, deliberately pending sentinel.
    fixture.extra_gate_set(0)
    fixture.extra_gate_wait(unrelated.cuda_stream)
    assert fixture.stream_pending(unrelated.cuda_stream)
    if mode == "failed_tail":
        gpu.trim()
        assert fixture.stream_pending(unrelated.cuda_stream)
        assert gpu.stats()["weights_resident"] == resident
        assert api.retirement_stats()["quarantined"] == quarantine + 1
        observations["retained_counts"] = counts(fixture, jobs=1)
        busy(fixture)
        checks.append(
            "final alias dropped in capture quarantines exact Pin and retains Source custody"
        )
        checks.append(
            "trim skips quarantined storage while independent unrelated gate stays pending"
        )
        observations["retained_weight_bytes"] = resident
        observations["intentional_quarantine_until_process_exit"] = True
    else:
        observations["physical_baseline"] = trim_baseline(
            gpu, fixture, unrelated, baseline
        )
        observations["final_counts"] = settled(fixture, deadline, quarantine)
        assert fixture.stream_pending(unrelated.cuda_stream)
    fixture.extra_gate_set(1)
    unrelated.synchronize()
    if mode != "failed_tail":
        fixture.close()
    else:
        busy(
            fixture
        )  # Opening a gate never converts quarantine into checked completion.
    observations["retirement_after_capture"] = api.retirement_stats()
    return checks, observations


def run(provider: str, device: int, gpu_uuid: str, mode: str, timeout_s: float = 60):
    if mode not in MODES or not math.isfinite(timeout_s) or timeout_s <= 0:
        raise ValueError("choose a supported mode and a positive explicit deadline")
    deadline = time.monotonic() + timeout_s
    vt.install(device)
    torch.cuda.set_device(device)
    actual_uuid = str(torch.cuda.get_device_properties(device).uuid)
    assert uuid_text(actual_uuid) == uuid_text(gpu_uuid), (actual_uuid, gpu_uuid)
    streams = tuple(torch.cuda.Stream(device=device) for _ in range(5))
    gpu = api.open(device)
    quarantine = api.retirement_stats()["quarantined"]
    initial = gpu.stats()
    assert initial["weights_resident"] == 0, initial
    outputs, scratch, expected = warm(device, streams)
    gpu.trim()
    baseline = gpu.stats()
    poll(
        deadline,
        lambda: not api.retirement_stats()["queued"],
        "initial native retirement",
    )
    provider_owner = Provider(provider)
    fixture = provider_owner.create(
        gpu, NBYTES, mode=mode if mode in ("partial", "prepare_error") else "ordering"
    )
    # Provider keeps the helper rooted until checked Fixture.close succeeds;
    # a quarantined fixture therefore stays owned through process exit.
    if mode == "ordering":
        checks, observations = ordering(
            gpu, fixture, streams, outputs, expected, baseline, deadline, quarantine
        )
    elif mode in ("partial", "prepare_error"):
        checks, observations = failure(
            gpu, fixture, streams, baseline, deadline, quarantine, mode
        )
    else:
        checks, observations = capture_case(
            gpu,
            fixture,
            streams,
            outputs,
            scratch,
            expected,
            baseline,
            deadline,
            quarantine,
            mode,
        )
    # Every gate is open before any ordinary stream/resource cleanup. Failure
    # paths deliberately propagate to the parent's owned-child protocol; they
    # do not improvise a global synchronize or free uncertain native resources.
    del outputs, scratch
    gc.collect()
    for stream in streams:
        stream.synchronize()
    gpu.trim()
    final = gpu.stats()
    assert final["working_live"] == initial["working_live"], (initial, final)
    if mode != "failed_tail":
        assert final["weights_resident"] == initial["weights_resident"], (
            initial,
            final,
        )
    for stream in streams:
        vt.retire_stream(stream, device)
    gpu.close()
    return {
        "passed": True,
        "mode": mode,
        "gpu_uuid": actual_uuid,
        "pid": os.getpid(),
        "device": device,
        "torch": str(torch.__version__),
        "checks": checks,
        "observations": observations,
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
    print(
        json.dumps(
            run(args.provider, args.device, args.gpu_uuid, args.mode, args.timeout_s)
        )
    )
