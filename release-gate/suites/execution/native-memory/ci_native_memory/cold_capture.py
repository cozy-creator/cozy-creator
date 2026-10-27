"""Real GPU work in an actual kernel-ready opener versus prepared/ordinary capture."""

from __future__ import annotations

import json
import os
import threading
import time

import msgspec
from cozy_runtime.author import Context


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 3284001


class Report(msgspec.Struct):
    passed: bool
    detail: str


def gpu(payload: Probe, ctx: Context) -> Report:
    import torch
    from cozy_runtime.author import CapabilityError
    from cozy_runtime.author._model import Building
    from cozy_runtime.compute import PreparedRepeat
    from cozy_runtime.graphs import domain
    from cozy_runtime.internal import int8_linear, kernel_ready

    device = torch.device(str(ctx.device))
    index = device.index or 0
    entered, attempted, ended = threading.Event(), threading.Event(), threading.Event()
    results = {}
    name = f"real-cold-int8-{payload.seed} cuda:{index}"

    def open_real_kernel(wait):
        assert wait
        # Actual bounded GPU numerical self-check, not a mock opener or fixed sleep.
        # Keep exercising GPU APIs until the consumer has tested eligibility, so the
        # race is controlled by work ownership rather than scheduler timing.
        try:
            with torch.cuda.device(device), torch.no_grad():
                a = torch.eye(512, device=device, dtype=torch.float32)
                iterations = 0
                entered.set()
                deadline = time.monotonic() + 30
                while not attempted.is_set():
                    product = a @ a
                    torch.cuda.current_stream(device).synchronize()
                    assert torch.equal(product, a)
                    iterations += 1
                    if time.monotonic() > deadline:
                        raise TimeoutError("capture eligibility did not settle")
                results["real_gpu_iterations"] = iterations
                return int8_linear._library(torch, index, True)
        finally:
            ended.set()

    with torch.inference_mode(False):
        parent = torch.arange(256, device=device, dtype=torch.float32)
        state = parent.clone()
    assert kernel_ready.ready(name, open_real_kernel) is None
    assert entered.wait(30), "actual opener never started"
    owner = domain.Domain(torch, None)
    refused = []
    try:
        try:
            owner.initialize()
        except Building:
            refused.append("domain")
        else:
            raise AssertionError("domain capture entered beside an active GPU opener")
        assert owner.stream is None and not owner.closed
        try:
            PreparedRepeat.capture(lambda _: state.add_(2), state=(state,), steps=3)
        except CapabilityError as error:
            assert error.code == "prepared_graph_ineligible"
            refused.append("prepared")
        else:
            raise AssertionError("prepared capture entered beside an active GPU opener")
        assert torch.equal(state, parent), "refused preparation changed the input"
    finally:
        attempted.set()
        assert ended.wait(60), "real kernel opener failed to drain"
    # The opener has returned; wait for its existing slot publication without invoking
    # a second opener. No GPU work remains on that thread after it publishes.
    deadline = time.monotonic() + 5
    while kernel_ready.pending().get(name) == "opening":
        if time.monotonic() > deadline:
            raise TimeoutError("kernel slot did not publish")
        time.sleep(0.001)
    kernel_ready.boundary()
    assert kernel_ready.ready(name) is not None, kernel_ready.failures(name)
    # Real cold library is now verified; ordinary owner initialization and prepared
    # replay both succeed in the same executor without switching capture error modes.
    owner.initialize()
    owner.finish()
    plan = PreparedRepeat.capture(lambda _: state.add_(2), state=(state,), steps=3)
    try:
        (actual,) = ctx.plan(plan).wait()
        assert torch.equal(actual, parent + 6)
    finally:
        plan.close()
    assert kernel_ready._OPENERS == 0
    return Report(
        True,
        json.dumps(
            {
                "pid": os.getpid(),
                "refused": refused,
                "real_gpu_iterations": results["real_gpu_iterations"],
                "checked_native_library": True,
                "successor_exact": True,
                "scope": "real GPU opener exclusion and same-process successor; no benchmark",
            },
            sort_keys=True,
        ),
    )
