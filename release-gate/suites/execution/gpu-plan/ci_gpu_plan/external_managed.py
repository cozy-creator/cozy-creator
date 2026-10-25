"""Resident managed work; only its real request cancellation stops the prepared GPU loop."""

import json
import os
import threading
import time

import torch
from cozy_runtime.compute import PreparedRepeat
from tensorfs import varena


def exercise(ctx, tel, weight_mib):
    def report(**fields):
        tel.log(json.dumps(fields))

    torch.set_num_threads(1)
    if not 256 <= weight_mib <= 1024:
        raise ValueError("managed fixture must retain 256..1024 MiB")
    varena.install_allocator()
    arena = varena.Arena(0)
    region = arena.reserve(weight_mib << 20)
    use = region.prepare()
    storage = use.tensor
    storage.zero_()  # Complete immutable bytes, including the unused extent.
    weight = storage[: 256 * 256 * 4].view(torch.float32).reshape(256, 256)
    weight.fill_(0.001)
    use.commit()
    del use
    state = torch.ones((16, 256), device="cuda") * 0.01

    def step(index, weight=weight):
        state.copy_(torch.tanh(state @ weight + 0.001))

    plan = PreparedRepeat.capture(step, state=(state,), steps=2**32 - 1)
    execution = ctx.plan(plan)
    assert plan.facts()["host_graph_launches"] == 1
    finished = threading.Event()

    def deadline():
        if not finished.wait(150):
            execution.cancel()  # safety bound; cannot produce a passing monitor receipt

    threading.Thread(target=deadline, daemon=True).start()
    report(
        pressure_ready=True,
        pid=os.getpid(),
        native_bytes=weight_mib << 20,
        at_ns=time.time_ns(),
    )
    try:
        execution.wait()  # Context cancellation sends cooperative control and joins.
    finally:
        finished.set()
        plan.close()
        progress = execution.poll()
        del step, weight, storage, region
        assert arena.stats()["pinned_bytes"] == 0, arena.stats()
        before = torch.cuda.mem_get_info()[0]
        del arena  # release this fixture's retained native cache after checked completion
        torch.cuda.empty_cache()
        released = torch.cuda.mem_get_info()[0] - before
        assert (
            progress.done
            and progress.canceled
            and 0 < progress.completed_steps < plan.steps
        )
        report(
            pressure_cleanup=True,
            pinned_bytes=0,
            freed_bytes=released,
            steps=progress.completed_steps,
            at_ns=time.time_ns(),
        )
    raise RuntimeError("safety deadline elapsed without request cancellation")
