"""Resident managed work; only its real request cancellation stops the prepared GPU loop."""

import json
import os
import sys
import threading
import time

import torch
from cozy_runtime.compute import PreparedRepeat
from tensorfs import varena


def report(**fields):
    print(json.dumps(fields), flush=True)


def main():
    torch.set_num_threads(1)
    varena.install_allocator()
    arena = varena.Arena(0)
    region = arena.reserve(512 << 20)
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
    execution = plan.submit()
    assert plan.facts()["host_graph_launches"] == 1
    stop = threading.Event()

    def stopping():
        sys.stdin.readline()
        stop.set()

    threading.Thread(target=stopping, daemon=True).start()
    report(
        pressure_ready=True,
        pid=os.getpid(),
        native_bytes=512 << 20,
        at_ns=time.time_ns(),
    )
    automatic = False
    try:
        deadline = time.monotonic() + 150
        while time.monotonic() < deadline:
            if stop.wait(0.25):
                automatic = True
                break
            report(pressure_heartbeat=True)
    finally:
        execution.cancel()
        execution.wait()
        progress = execution.poll()
        plan.close()
        del step, weight, storage, region
        torch.cuda.empty_cache()
    assert automatic, "monitor did not cancel within the finite safety deadline"
    assert (
        progress.done
        and progress.canceled
        and 0 < progress.completed_steps < plan.steps
    )
    assert arena.stats()["resident_bytes"] == 0, arena.stats()
    report(
        pressure_cleanup=True,
        native_bytes=0,
        steps=progress.completed_steps,
        at_ns=time.time_ns(),
    )


if __name__ == "__main__":
    main()
