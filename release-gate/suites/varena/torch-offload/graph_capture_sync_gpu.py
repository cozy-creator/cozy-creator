"""Real scoped capture-helper controls; coordinator GPU execution only.

Exact pool/reserve, graphs, buffers and copy kernels are prepared BEFORE the
unrelated producer gate. Preparation joins caller -> capture, not the device;
native preparation/recovery is not claimed globally nonblocking.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import threading
import time
from pathlib import Path

import torch
from tensorfs.varena import Source, api
from tensorfs.varena import torch as vt


def preparation_ordering(device, capture, fixture, pool, reserve, timeout_s):
    """Real producer -> caller -> capture dependencies, with a mapped host gate."""
    producer = torch.cuda.Stream(device=device)
    caller = torch.cuda.Stream(device=device)
    value = torch.zeros(32, dtype=torch.uint8, device=device)
    output = torch.empty_like(value)
    graph = torch.cuda.CUDAGraph()
    producer.wait_stream(capture)
    fixture.extra_gate_set(0)
    fixture.extra_gate_wait(producer.cuda_stream)
    with torch.cuda.stream(producer):
        value.fill_(91)
    value.record_stream(producer)
    caller.wait_stream(producer)  # Other producers are an explicit caller obligation.
    returned, result = threading.Event(), {}

    def attempt():
        try:
            with (
                torch.cuda.device(device),
                torch.cuda.stream(caller),
                vt.graph(graph, pool=pool, stream=capture, reserve_bytes=reserve),
            ):
                output.copy_(value)
        except BaseException as error:  # noqa: BLE001 - propagate thread failures to its owner
            result["error"] = error
        finally:
            returned.set()

    thread = threading.Thread(target=attempt)
    thread.start()
    try:
        deadline = time.monotonic() + timeout_s
        while not fixture.stream_pending(capture.cuda_stream):
            assert not returned.is_set(), result
            if time.monotonic() >= deadline:
                raise TimeoutError("capture preparation never ordered the caller gate")
            time.sleep(0.005)
        assert fixture.stream_pending(producer.cuda_stream)
        assert not returned.is_set(), result
    finally:
        fixture.extra_gate_set(1)
        thread.join(timeout_s)
        assert not thread.is_alive(), "coordinator must terminate failed isolated child"
    if "error" in result:
        raise result["error"]
    with vt.replay_scope(device):
        vt.replay(graph, stream=capture)
    capture.synchronize()
    assert torch.equal(output.cpu(), torch.full((32,), 91, dtype=torch.uint8))
    graph.reset()
    value.record_stream(capture)
    return {"producer_caller_capture_ordering": True, "readback": 91}


def run(device, provider_path, provider_python, timeout_s):
    if not hasattr(api.Use, "wait_ready"):
        raise NotImplementedError(
            "scoped capture helper and readiness must be integrated"
        )
    spec = importlib.util.spec_from_file_location(
        "capture_sync_source_provider", provider_python
    )
    provider_module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(provider_module)
    vt.install(device)
    gpu = api.open(device)
    capture = torch.cuda.Stream(device=device)
    side = torch.cuda.Stream(device=device)
    with torch.cuda.device(device):
        provider = provider_module.Provider(provider_path)
        fixture = provider.create(gpu, 32, mode="ordering")
    reserve = 2 << 20
    pool = tuple(torch.cuda.graph_pool_handle())
    warm_graph = torch.cuda.CUDAGraph()
    failed_graph = torch.cuda.CUDAGraph()
    scoped_graph = torch.cuda.CUDAGraph()
    pending = None
    thread = None
    result = {}
    returned = threading.Event()
    quarantine = api.retirement_stats()["quarantined"]
    with torch.inference_mode(), torch.cuda.stream(capture):
        source = gpu.region(Source.from_tensor(torch.arange(32, dtype=torch.uint8)))
        selected = gpu.use([source], capture.cuda_stream)
        weight = selected.tensor(dtype="uint8", shape=(32,))
        selected.wait_ready()
        output = torch.empty(32, dtype=torch.uint8, device=device)
        output.copy_(weight)
        capture.synchronize()
        # Ordinary caller compatibility is exercised before unrelated work is armed.
        with vt.graph(warm_graph, pool=pool, stream=capture, reserve_bytes=reserve):
            output.copy_(weight)
        with vt.replay_scope(device):
            vt.replay(warm_graph, stream=capture)
        capture.synchronize()
        assert torch.equal(output.cpu(), torch.arange(32, dtype=torch.uint8))
        vt.prepare_pool(
            pool, reserve, device
        )  # Exact existing have>=wanted path later.
        capture.synchronize()
        ordering = preparation_ordering(
            device, capture, fixture, pool, reserve, timeout_s
        )
        # A real body exception must end capture/routing and preserve the live pool.
        try:
            with vt.graph(
                failed_graph, pool=pool, stream=capture, reserve_bytes=reserve
            ):
                output.copy_(weight)
                raise RuntimeError("intentional capture-helper body failure")
        except RuntimeError as error:
            assert str(error) == "intentional capture-helper body failure", str(error)
        else:
            raise AssertionError("actual capture body failure was swallowed")
        assert not torch.cuda.is_current_stream_capturing()
        failed_graph.reset()

        def attempt_scoped_capture():
            try:
                with (
                    torch.cuda.device(device),
                    torch.cuda.stream(capture),
                    vt.graph(
                        failed_graph,
                        pool=pool,
                        stream=capture,
                        reserve_bytes=reserve,
                    ),
                ):
                    output.copy_(weight)
            except RuntimeError as error:
                result["error"] = str(error)
            finally:
                returned.set()

        try:
            pending = gpu.use([fixture.region], side.cuda_stream)
            assert fixture.source_pending() and fixture.stream_pending(side.cuda_stream)
            thread = threading.Thread(target=attempt_scoped_capture)
            thread.start()
            assert returned.wait(timeout_s), (
                "scoped helper did not reach admission while unrelated producer remained gated; do not infer globally nonblocking preparation"
            )
            assert (
                "native producers must finish before CUDA graph capture"
                in result.get("error", "")
            ), result
            assert fixture.source_pending() and fixture.stream_pending(side.cuda_stream)
            assert api.retirement_stats()["quarantined"] == quarantine
            with torch.cuda.stream(capture):
                assert not torch.cuda.is_current_stream_capturing()
        finally:
            fixture.gate_set(1)
            fixture.extra_gate_set(1)
            if thread is not None:
                thread.join(timeout_s)
                assert not thread.is_alive(), (
                    "coordinator must terminate this failed isolated child"
                )
        pending.wait_ready()
        pending.close()
        # Failed admission must preserve the existing graph's live pool.
        output.zero_()
        with vt.replay_scope(device):
            vt.replay(warm_graph, stream=capture)
        capture.synchronize()
        assert torch.equal(output.cpu(), torch.arange(32, dtype=torch.uint8))
        with vt.graph(
            scoped_graph,
            pool=pool,
            stream=capture,
            reserve_bytes=reserve,
        ):
            output.copy_(weight)
        # Shared-pool successful definitions replay in capture order, serially.
        for graph in (warm_graph, scoped_graph):
            output.zero_()
            with vt.replay_scope(device):
                vt.replay(graph, stream=capture)
            capture.synchronize()
            assert torch.equal(output.cpu(), torch.arange(32, dtype=torch.uint8))
        for graph in (failed_graph, scoped_graph, warm_graph):
            graph.reset()
        weight.record_stream(capture)
        selected.close()
        weight = None
        capture.synchronize()
        source.close()
        fixture.region.close()
        deadline = time.monotonic() + timeout_s
        while fixture.counts()["source_dropped"] != 1:
            if time.monotonic() >= deadline:
                raise TimeoutError("explicit own Source retirement deadline")
            time.sleep(0.005)
        fixture.close()
        gpu.close()
    assert api.retirement_stats()["quarantined"] == quarantine
    print(
        json.dumps(
            {
                "gpu_executed": True,
                "torch": torch.__version__,
                "mode": "capture_helper_scoped_sync",
                "ordinary_caller_compatibility": True,
                "preparation_ordering": ordering,
                "actual_capture_body_failure_cleanup": True,
                "scoped_existing_pool_refuses_unrelated_producer_without_draining_it": True,
                "failure_cleanup_preserves_live_pool": True,
                "scoped_success_replay": True,
                "native_prepare_globally_nonblocking": "not claimed",
                "error": result["error"],
            }
        )
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--device", type=int, default=0)
    parser.add_argument("--provider", type=Path, required=True)
    parser.add_argument("--provider-python", type=Path, required=True)
    parser.add_argument("--timeout-s", type=float, default=60)
    args = parser.parse_args()
    run(args.device, args.provider, args.provider_python, args.timeout_s)
