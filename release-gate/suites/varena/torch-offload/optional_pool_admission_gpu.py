"""Prepared real native optional-pool refusal with an otherwise fitting eager body.

Coordinator-only isolated ordinary-Cozy child. No fake pool, patched method or
allocation menu. The target is the native measured packing capacity, unmodified.
"""

from __future__ import annotations

import argparse
import json

import torch
from tensorfs.varena import torch as vt
from tensorfs.varena._offload import OffloadError
from tensorfs.varena._offload_auto import _Preparation


def run(device):
    vt.install(device)
    vt.initialize(device)
    stream = torch.cuda.Stream(device=device)
    calls = []

    def authored(value):
        calls.append(
            True
        )  # Observe real authored execution, not an allocator stand-in.
        return value + 1

    with torch.inference_mode(), torch.cuda.stream(stream):
        value = torch.arange(32, dtype=torch.float32, device=device)
        warm = authored(value)
        stream.synchronize()
        expected = warm.cpu()
        del warm
        stream.synchronize()
        token = vt.phase_enter(device, "auto-pool-real-admission", mode="eager")
        completed = False
        try:
            measured = authored(value)
            stream.synchronize()
            completed = True
        finally:
            observation = vt.phase_leave(device, token, completed=completed)
        assert observation["published"] and observation["successful_peak_bytes"] > 0
        native_target = observation["successful_capture_pool_bytes"]
        assert native_target >= observation["successful_peak_bytes"]
        torch.testing.assert_close(measured.cpu(), expected, rtol=0, atol=0)
        del measured
        stream.synchronize()
        vt.trim(device)
        # Input keeps its existing Working page live; a tiny eager output can
        # reuse that page, whereas a separate private graph pool needs backing.
        baseline = vt.stats(device)
        assert baseline["mapped_bytes"] > 0
        pool = tuple(torch.cuda.graph_pool_handle())
        attempt = _Preparation()
        selected_before_body = len(calls)
        try:
            vt.set_limit(device, baseline["mapped_bytes"])
            admitted = attempt.prepare(device, pool, native_target)
            assert admitted is False and attempt.capacity_refused
            assert attempt.can_choose_eager
            assert not attempt.body_started and len(calls) == selected_before_body
            assert attempt.pool is None and attempt.failure is None
            attempt.mark_body_started()
            assert not attempt.can_choose_eager
            fallback = authored(value)  # First/only body of THIS invocation.
            stream.synchronize()
            torch.testing.assert_close(fallback.cpu(), expected, rtol=0, atol=0)
            assert len(calls) == selected_before_body + 1
            try:
                attempt.prepare(device, pool, native_target)
            except OffloadError as error:
                assert "precede authored execution" in str(error)
            else:
                raise AssertionError(
                    "strategy selection resumed after an authored body"
                )
            del fallback
        finally:
            attempt.cancel()
            vt.set_limit(device, None)
        stream.synchronize()
        vt.trim(device)
        # Native capacity is restored; the exact same target can be admitted
        # with a fresh preparation owner, without resizing/retrying a request.
        later = _Preparation()
        try:
            assert later.prepare(device, pool, native_target)
        finally:
            later.cancel()
        del value
        stream.synchronize()
        vt.trim(device)
        vt.phase_forget(device, "auto-pool-real-admission")
    print(
        json.dumps(
            {
                "gpu_executed": True,
                "torch": torch.__version__,
                "native_measured_target": native_target,
                "native_facts_budget_for_test": baseline["mapped_bytes"],
                "private_pool_capacity_refused_before_body": True,
                "otherwise_supported_eager_request_ran_once": True,
                "post_body_strategy_selection_refused": True,
                "unsupported_refusal_or_full_automatic_controller": "not exercised",
            }
        )
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--device", type=int, default=0)
    run(parser.parse_args().device)
