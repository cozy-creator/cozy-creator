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


class GateOwner:
    """Independent fixture root; always open the gate before cleanup joins."""

    def __init__(self, gate, timeout):
        self.gate = gate
        self.requested, self.opened = threading.Event(), threading.Event()
        self.errors = []
        self.worker = threading.Thread(target=self._release, args=(timeout,))
        self.worker.start()

    def _release(self, timeout):
        try:
            if not self.requested.wait(timeout):
                self.errors.append(TimeoutError("fixture gate-release deadline"))
        finally:
            try:
                self.gate.gate_set(1)
            except BaseException as error:
                self.errors.append(error)
                raise
            finally:
                self.opened.set()

    def release(self, timeout):
        self.requested.set()
        self.worker.join(timeout)
        assert not self.worker.is_alive(), "gate owner did not finish release"
        if self.errors:
            raise self.errors[0]


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
    gate_owner = None
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
        gate_owner = GateOwner(gate, args.timeout_s)
        try:
            replay_stream = torch.cuda.Stream(device=args.device)
            replay_stream.wait_stream(stream)
            gate.gate_wait(replay_stream.cuda_stream)
            with torch.no_grad(), torch.cuda.stream(replay_stream):
                result = model(value)  # Real cached launch/copies behind gate.
            assert gate.stream_pending(replay_stream.cuda_stream)
        except BaseException:
            gate_owner.release(args.timeout_s)
            raise
    before = vt.stats(args.device)
    # Only result/input/actual stream/gate/weak diagnostics leave this scope.
    # No owner.close(), cache request or graph.reset() is performed.
    return refs, result, expected, value, stream, replay_stream, gate_owner, before


def run(args):
    quarantine = api.retirement_stats()["quarantined"]
    refs, result, expected, _value, _stream, replay_stream, gate_owner, before = create(
        args
    )
    gate = gate_owner.gate if gate_owner is not None else None
    probe = api.open(args.device)  # Observer only; does not reference Python owner.
    started = threading.Event()
    errors, observations = [], {}

    def collect():
        started.set()
        try:
            with torch.cuda.device(args.device):
                gc.collect()
                observations["first_collection_before_gate_release"] = (
                    gate_owner is not None and not gate_owner.opened.is_set()
                )
                dead = {
                    name: all(ref() is None for ref in group)
                    for name, group in refs.items()
                }
                if gate_owner is not None and not all(dead.values()):
                    # Reset/GC may wait for consumers; no nonblocking promise.
                    assert gate_owner.opened.wait(args.timeout_s)
                    replay_stream.synchronize()
                    gc.collect()
                dead = {
                    name: all(ref() is None for ref in group)
                    for name, group in refs.items()
                }
                assert all(dead.values()), dead  # BEFORE trim/cache pressure.
                observations["collected_without_close_or_pressure"] = dead
                observations["trim_started_before_gate_release"] = (
                    gate_owner is not None and not gate_owner.opened.is_set()
                )
                probe.trim()  # Checked trim may also wait for consumer completion.
                vt.trim(args.device)
        except BaseException as error:
            errors.append(error)
            raise

    worker = threading.Thread(target=collect)
    pending = None
    try:
        if gate is not None:
            assert gate.stream_pending(replay_stream.cuda_stream)
            pending = vt.stats(args.device)
            assert before["pools_bytes"] > 0
            assert pending["pools_bytes"] >= before["pools_bytes"]
            assert gate.stream_pending(replay_stream.cuda_stream)
        worker.start()
        assert started.wait(args.timeout_s), "collection worker did not enter"
    finally:
        # The gate has an independent owner, also during native/driver waits.
        # Never join collection/reset/trim while its consumers remain gated.
        try:
            if gate_owner is not None:
                gate_owner.release(args.timeout_s)
        finally:
            if worker.ident is not None:
                worker.join(args.timeout_s)
    assert not worker.is_alive()
    if errors:
        raise errors[0]
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
        **observations,
        "cleanup_nonblocking_not_required": True,
        "before": before,
        "before_cleanup_while_gate_pending": pending,
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
