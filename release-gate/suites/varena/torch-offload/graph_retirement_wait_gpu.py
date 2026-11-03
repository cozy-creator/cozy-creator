"""Prepared actual graph capture over Ready bytes with real Job retirement held.

Coordinator-only GPU execution in an isolated ordinary-Cozy child. Existing C
Source/mapped gate/retirement mutexes are used unchanged; no native owner substitute.
"""

from __future__ import annotations

import argparse
import gc
import importlib.util
import json
import sys
import threading
import time
from pathlib import Path

import torch
from tensorfs.varena import api
from tensorfs.varena import torch as vt
from tensorfs.varena._offload_graph import _UNPROVEN


def poll(deadline, observe, label):
    while time.monotonic() < deadline:
        result = observe()
        if result:
            return result
        time.sleep(0.005)
    raise TimeoutError("explicit fixture deadline: " + label)


class Model(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.weight = torch.nn.Parameter(
            torch.full((32,), 73, dtype=torch.uint8), requires_grad=False
        )
        self.body_calls = []
        self.retirement_counts = None

    def forward(self, value):
        # Observe the actual warm/capture body, without substituting dispatch.
        counts = self.retirement_counts()  # Existing helper reads CPU atomics only.
        assert counts["job_dropped"] == 1, counts
        self.body_calls.append(torch.cuda.is_current_stream_capturing())
        return self.weight + value


def run(args):
    deadline = time.monotonic() + args.timeout_s
    spec = importlib.util.spec_from_file_location(
        "graph_ready_retirement_provider", args.provider_python
    )
    provider_module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(provider_module)
    model = Model().eval()
    owner = vt.offload(model, device=args.device)
    gpu = owner._owner.gpu
    producer = torch.cuda.Stream(device=args.device)
    capture = torch.cuda.Stream(device=args.device)
    with torch.cuda.device(args.device):
        provider = provider_module.Provider(args.provider)
        fixture = provider.create(gpu, 32, mode="ordering")
    model.retirement_counts = fixture.counts
    quarantine = api.retirement_stats()["quarantined"]
    version = model.weight._version_owner
    assert version.source.data == bytes([73]) * 32
    # Explicit byte-identical public Source adapter, as in graph_epochs_gpu.
    # This is not general public Source-map adoption.
    version.region = fixture.region
    entered, returned = threading.Event(), threading.Event()
    outcome, wait_calls = {}, []
    worker = None
    held = False
    with torch.inference_mode(), torch.cuda.stream(capture):
        example = torch.full((32,), 2, dtype=torch.uint8, device=args.device)
        capture.synchronize()
    try:
        fixture.job_retirement_hold(True)
        held = True
        # Publish bytes BEFORE graph construction. Its later Gpu.use receives
        # Ready Pins, so readiness must inspect Region control, not Pin.submission.
        published = gpu.use([fixture.region], producer.cuda_stream)
        exported = published.tensor(
            dtype="uint8", shape=(32,), stream=producer.cuda_stream
        )
        with torch.inference_mode(), torch.cuda.stream(producer):
            copied = exported.clone()
        assert fixture.source_pending() and fixture.stream_pending(producer.cuda_stream)
        fixture.gate_set(1)
        producer.synchronize()
        pending_before = poll(
            deadline, fixture.job_retirement_pending, "actual Job VR_BUSY"
        )
        observed = fixture.counts()
        assert observed["prepares"] == observed["submits"] == 1, observed
        assert observed["job_retired"] == observed["job_dropped"] == 0, observed
        assert torch.equal(copied.cpu(), torch.full((32,), 73, dtype=torch.uint8))

        def construct():
            def profile(frame, event, _):
                if event == "call" and frame.f_code is api.Use.wait_ready.__code__:
                    wait_calls.append(frame.f_locals["self"]._handle)
                    entered.set()  # Python method entry, not proof of C ABI admission.

            previous = sys.getprofile()
            sys.setprofile(profile)
            try:
                with torch.inference_mode(), torch.cuda.stream(capture):
                    outcome["segment"] = owner.capture(
                        model, (example,), stream=capture, reserve_bytes=2 << 20
                    )
            except Exception as error:  # noqa: BLE001 - joined owner propagates real errors
                outcome["error"] = error
            finally:
                sys.setprofile(previous)
                returned.set()

        worker = threading.Thread(
            target=construct, name="graph-retirement-wait", daemon=True
        )
        worker.start()
        poll(deadline, entered.is_set, "actual public wait_ready Python call")
        assert wait_calls and published._handle not in wait_calls
        assert not model.body_calls and not returned.is_set(), outcome
        held_counts = fixture.counts()
        assert held_counts["prepares"] == held_counts["submits"] == 1, held_counts
        assert held_counts["job_retired"] == held_counts["job_dropped"] == 0, (
            held_counts
        )
        assert fixture.job_retirement_pending() >= pending_before
        # Release the independent positive-control export before permitting
        # graph completion/Pins0 classification. Never force native Pin release.
        exported.record_stream(producer)
        published.close()
        del exported, published, copied
        gc.collect()
    finally:
        fixture.gate_set(1)
        if held:
            fixture.job_retirement_hold(False)  # Same thread acquired this mutex.
        if worker is not None:
            worker.join(max(0, deadline - time.monotonic()))
            if worker.is_alive():
                raise TimeoutError(
                    "coordinator must terminate this failed isolated child"
                )
    if "error" in outcome:
        raise outcome["error"]
    segment = outcome["segment"]
    assert returned.is_set() and model.body_calls == [False, True], model.body_calls
    assert fixture.counts()["job_dropped"] == 1
    assert all(entry["pins"] == 0 for entry in segment._weight_entries())
    with torch.inference_mode(), torch.cuda.stream(capture):
        result = segment(example, stream=capture)
        capture.synchronize()
        assert torch.equal(result.cpu(), torch.full((32,), 75, dtype=torch.uint8))
    assert len(wait_calls) == 1  # Profile covered constructor only, not replay.
    assert model.body_calls == [False, True], "replay re-executed the authored model"
    segment.close()
    owner.close()
    outcome.clear()
    segment = result = model = owner = version = None
    gc.collect()
    fixture.region.close()
    poll(
        deadline,
        lambda: fixture.counts()["source_dropped"] == 1,
        "own Source retirement",
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
                "selected_bytes_completed_before_graph_construction": True,
                "actual_job_busy_before_graph_wait": pending_before,
                "counts_while_retirement_held": held_counts,
                "graph_body_not_entered_while_selected_retirement_held": True,
                "warm_and_capture_bodies_observed_real_job_dropped": True,
                "captured_after_retirement_release_and_replay_exact": True,
                "python_wait_method_entry_observed": True,
                "native_wait_entry_proven": False,
                "late_retirement_error_or_replay_refill_wait": "not exercised",
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
