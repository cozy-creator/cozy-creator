"""Coordinator-only real graph epoch controls using the existing C Source helper.

Run one isolated ordinary-Cozy child per mode. No timed CUDA delay or mocked owner.
The provider Python module/library are explicit matched-cohort inputs.
"""

from __future__ import annotations

import argparse
import gc
import importlib.util
import json
import threading
import time
from pathlib import Path

import torch
from tensorfs import _ext
from tensorfs.varena import api
from tensorfs.varena import torch as vt
from tensorfs.varena._offload_graph import _UNPROVEN, GraphInvalidated

parser = argparse.ArgumentParser()
parser.add_argument("--device", type=int, default=0)
parser.add_argument("--provider", type=Path, required=True)
parser.add_argument("--provider-python", type=Path, required=True)
parser.add_argument(
    "--mode",
    choices=("tail", "reset", "source_failure", "capture_refusal"),
    required=True,
)
parser.add_argument("--timeout-s", type=float, default=60)
args = parser.parse_args()
if args.mode == "capture_refusal":
    # Isolate only the convenience-sync regression: exact pool/work is prepared
    # before the gate. Fresh high-level preparation is allowed to wait/recover.
    from graph_capture_sync_gpu import run

    run(args.device, args.provider, args.provider_python, args.timeout_s)
    raise SystemExit(0)
if not hasattr(api.Use, "wait_ready"):
    raise NotImplementedError(
        "graph epochs require scoped readiness/capture helper integration"
    )
spec = importlib.util.spec_from_file_location(
    "graph_real_source_provider", args.provider_python
)
provider_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(provider_module)


class BytesModel(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.weight = torch.nn.Parameter(
            torch.full((32,), 73, dtype=torch.uint8), requires_grad=False
        )

    def forward(self, value):
        return self.weight + value


def poll(observe, label):
    deadline = time.monotonic() + args.timeout_s
    while True:
        value = observe()
        if value:
            return value
        if time.monotonic() >= deadline:
            raise TimeoutError("explicit fixture deadline: " + label)
        time.sleep(0.005)


def finish_fixture(fixture):
    fixture.gate_set(1)
    fixture.extra_gate_set(1)
    fixture.region.close()
    poll(lambda: fixture.counts()["source_dropped"] == 1, "own Source retirement")
    fixture.close()


model = BytesModel().eval()
owner = vt.offload(model, device=args.device)
gpu = owner._owner.gpu
stream = torch.cuda.Stream(device=args.device)
side = torch.cuda.Stream(device=args.device)
with torch.cuda.device(args.device):
    provider = provider_module.Provider(args.provider)
    fixture = provider.create(
        gpu, 32, mode="prepare_error" if args.mode == "source_failure" else "ordering"
    )
quarantine = api.retirement_stats()["quarantined"]
report = {"mode": args.mode, "torch": torch.__version__, "gpu_executed": True}

with torch.inference_mode(), torch.cuda.stream(stream):
    example = torch.ones(32, dtype=torch.uint8, device=args.device)
    example.add_(1)
    stream.synchronize()
    if args.mode == "source_failure":
        # Explicit fixture adapter: real foreign Region matches the exact immutable
        # CPU byte73 recipe. This is not general public Source-map support.
        assert model.weight._version_owner.source.data == bytes([73]) * 32
        model.weight._version_owner.region = fixture.region
        try:
            owner.capture(model, (example,), stream=stream, reserve_bytes=2 << 20)
        except api.SourceError:
            pass
        else:
            raise AssertionError("actual Source preparation failure was swallowed")
        assert fixture.counts()["prepares"] == 1, "application capture was retried"
        assert not _UNPROVEN, "pre-submission Source failure lost cleanup proof"
        owner.close()
        model.weight._version_owner.region = None
        finish_fixture(fixture)
        report["actual_source_failure_before_capture"] = True
    else:
        segment = owner.capture(model, (example,), stream=stream, reserve_bytes=2 << 20)
        assert all(e["pins"] == 0 for e in segment._weight_entries())
        entered, returned = threading.Event(), threading.Event()
        outcome = {}

        def after_launch(graph):
            fixture.gate_wait(stream.cuda_stream)
            entered.set()
            if args.mode == "reset":
                graph.reset()

        hook = segment._graph.register_replay_end_hook(after_launch)

        def invoke():
            try:
                with torch.inference_mode(), torch.cuda.stream(stream):
                    outcome["output"] = segment(example, stream=stream)
            except (RuntimeError, ValueError, AssertionError, TypeError) as error:
                outcome["error"] = error
            finally:
                returned.set()

        replay_thread = threading.Thread(target=invoke)
        replay_thread.start()
        try:
            assert entered.wait(args.timeout_s)
            assert fixture.stream_pending(stream.cuda_stream), (
                "consumer gate was not pending"
            )
            if args.mode == "tail":
                assert returned.wait(args.timeout_s), (
                    "host submission unexpectedly waited on consumer gate"
                )
                assert "error" not in outcome, outcome.get("error")
                assert all(
                    e["pins"] == 0 and not e["unjoined"]
                    for e in segment._weight_entries()
                )
                before_waits = _ext._varena_public_gpu_event_counts(gpu._handle)[2]
                trim_done = threading.Event()
                trimmed = {}

                def trim():
                    try:
                        trimmed["bytes"] = gpu.trim()
                    except (
                        RuntimeError,
                        ValueError,
                        AssertionError,
                        TypeError,
                    ) as error:
                        trimmed["error"] = error
                    finally:
                        trim_done.set()

                trim_thread = threading.Thread(target=trim)
                trim_thread.start()
                try:
                    poll(
                        lambda: (
                            _ext._varena_public_gpu_event_counts(gpu._handle)[2]
                            > before_waits
                        ),
                        "native last-consumer fence selection",
                    )
                    assert not trim_done.is_set() and fixture.stream_pending(
                        stream.cuda_stream
                    )
                finally:
                    fixture.gate_set(1)
                    trim_thread.join(args.timeout_s)
                assert trim_done.is_set() and "error" not in trimmed, trimmed
                report["native_trim_waited_actual_consumer_gate"] = True
                report["trimmed_bytes"] = trimmed["bytes"]
            else:
                # Reset may itself block under Torch/CUDA. The independent control
                # opens the gate; do not infer completion from a host thread marker.
                report["reset_returned_while_gate_pending"] = returned.is_set()
                fixture.gate_set(1)
        finally:
            fixture.gate_set(1)
            replay_thread.join(args.timeout_s)
            hook.remove()
        assert returned.is_set() and "error" not in outcome, outcome.get("error")
        stream.synchronize()
        assert torch.equal(
            outcome["output"].cpu(), torch.full((32,), 75, dtype=torch.uint8)
        )
        if args.mode == "reset":
            try:
                segment(example, stream=stream)
            except GraphInvalidated:
                pass
            else:
                raise AssertionError("reset graph admitted another execution")
            report["reset_inflight_output_preserved_and_new_selection_refused"] = True
        segment.close()
        owner.close()
        gc.collect()
        finish_fixture(fixture)
assert api.retirement_stats()["quarantined"] == quarantine
assert not _UNPROVEN
print(json.dumps(report))
