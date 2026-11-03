"""Actual automatic graph owner-drop/collection; coordinator-only GPU gate.

No explicit model close/cache-pressure request may break the tested ownership
chain. Optional real provider gate also checks in-flight backing/tail custody.
"""

from __future__ import annotations

import argparse
import gc
import importlib.util
import json
import threading
import time
import weakref
from pathlib import Path

import torch
from tensorfs.varena import api
from tensorfs.varena import torch as vt


def poll(timeout, observe, label):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = observe()
        if value:
            return value
        time.sleep(0.005)
    raise TimeoutError("graph resource collection gate: " + label)


def create(args):
    torch.set_float32_matmul_precision("highest")
    torch.manual_seed(734)
    model = torch.nn.Sequential(
        *[
            torch.nn.Sequential(torch.nn.Linear(32, 32), torch.nn.ReLU())
            for _ in range(2)
        ]
    ).eval()
    host = torch.arange(64.0).reshape(2, 32) / 64
    with torch.no_grad():
        expected = model(host)
    owner = vt.offload(model, device=args.device)
    stream = torch.cuda.Stream(device=args.device)
    with torch.no_grad(), torch.cuda.stream(stream):
        value = host.to(device=args.device)
        model(value)
        model(value)
        result = model(value)
        stream.synchronize()
        torch.testing.assert_close(result.cpu(), expected, rtol=2e-5, atol=2e-6)
    states = list(owner._automatic._states.values())
    assert len(states) == 2
    assert all(state.captures == 1 and state.replays == 2 for state in states)
    assert all(
        state.cache is not None and state.cache.segment is not None for state in states
    )
    segments = [state.cache.segment for state in states]
    assert all(
        not row["pins"] and not row["unjoined"]
        for segment in segments
        for row in segment._weight_entries()
    )
    assert all(
        segment._definition.resources is not segment._definition for segment in segments
    )
    refs = {
        "model": [weakref.ref(model)],
        "owner": [weakref.ref(owner)],
        "controller": [weakref.ref(owner._automatic)],
        "cache": [weakref.ref(state.cache) for state in states],
        "segment": [weakref.ref(segment) for segment in segments],
        "graph": [weakref.ref(segment._graph) for segment in segments],
        "resources": [
            weakref.ref(segment._definition.resources) for segment in segments
        ],
    }
    gate = None
    replay_stream = stream
    if args.mode == "inflight":
        spec = importlib.util.spec_from_file_location(
            "real_drop_gate", args.provider_python
        )
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with torch.cuda.device(args.device):
            provider = module.Provider(args.provider)
            gate = provider.create(owner._owner.gpu, 32, mode="ordering")
        replay_stream = torch.cuda.Stream(device=args.device)
        replay_stream.wait_stream(stream)
        gate.gate_wait(replay_stream.cuda_stream)
        with torch.no_grad(), torch.cuda.stream(replay_stream):
            result = model(value)  # Real cached launch/copies queued behind gate.
        assert gate.stream_pending(replay_stream.cuda_stream)
    before = vt.stats(args.device)
    # Only result/input/actual stream/gate/weak diagnostics leave this scope.
    # No owner.close(), cache request or graph.reset() is performed.
    return refs, result, expected, value, stream, replay_stream, gate, before


def run(args):
    quarantine = api.retirement_stats()["quarantined"]
    refs, result, expected, _value, _stream, replay_stream, gate, before = create(args)
    probe = api.open(args.device)  # Observer only; does not reference Python owner.
    collected = threading.Event()
    errors = []

    def collect():
        try:
            with torch.cuda.device(args.device):
                gc.collect()
        except BaseException as error:
            errors.append(error)
            raise
        finally:
            collected.set()

    worker = threading.Thread(target=collect)
    worker.start()
    try:
        assert collected.wait(args.timeout_s), "collection blocked before gate release"
        if errors:
            raise errors[0]
        dead = {
            name: all(ref() is None for ref in group) for name, group in refs.items()
        }
        assert all(dead.values()), dead  # BEFORE explicit trim or pressure.
        if gate is not None:
            assert gate.stream_pending(replay_stream.cuda_stream)
            probe.trim()
            vt.trim(args.device)
            pending = vt.stats(args.device)
            assert pending["pools_bytes"] >= before["pools_bytes"]
            assert gate.stream_pending(replay_stream.cuda_stream)
        else:
            pending = None
    finally:
        if gate is not None:
            gate.gate_set(1)
        worker.join(args.timeout_s)
    assert not worker.is_alive()
    replay_stream.synchronize()
    torch.testing.assert_close(result.cpu(), expected, rtol=2e-5, atol=2e-6)
    poll(
        args.timeout_s,
        lambda: api.retirement_stats()["queued"] == 0,
        "checked retirements",
    )
    probe.trim()
    vt.trim(args.device)
    after = vt.stats(args.device)
    assert after["pools_bytes"] < before["pools_bytes"]
    if gate is not None:
        gate.region.close()
        poll(
            args.timeout_s,
            lambda: gate.counts()["source_dropped"] == 1,
            "gate Source retirement",
        )
        gate.close()
    probe.close()
    assert api.retirement_stats()["quarantined"] == quarantine
    return {
        "passed": True,
        "mode": args.mode,
        "collected_without_close_or_pressure": dead,
        "before": before,
        "while_gate_pending": pending,
        "after": after,
        "exact_output_after_drop": True,
        "gpu_executed": True,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", type=int, default=0)
    parser.add_argument("--mode", choices=("idle", "inflight"), default="idle")
    parser.add_argument("--provider", type=Path)
    parser.add_argument("--provider-python", type=Path)
    parser.add_argument("--timeout-s", type=float, default=60)
    args = parser.parse_args()
    if args.mode == "inflight" and (
        args.provider is None or args.provider_python is None
    ):
        parser.error("inflight requires the existing matched real provider paths")
    torch.set_num_threads(1)
    torch.set_num_interop_threads(1)
    print(json.dumps(run(args), indent=2))
