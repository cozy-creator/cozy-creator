"""Real concurrent observers, cooperative close and size-targeted idle reclamation."""

from __future__ import annotations

import json
import sys
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import tensorfs
import torch
from cozy_runtime.compute import PreparedRepeat
from cozy_runtime.internal import accel, residency
from torch import nn


def main():
    generator = torch.Generator().manual_seed(int(sys.argv[1]))
    torch.set_num_threads(1)
    accel.initialize(torch, "cuda")
    memory = residency.Memory.open("cuda:0", budget_bytes=256 << 20)
    checkpoints, models, plans = [], {}, []
    rounds = []
    with tempfile.TemporaryDirectory(prefix="cozy-plan-pressure-") as directory:
        for name in ("active", "unrelated"):
            writer = tensorfs.Writer(Path(directory) / f"{name}.cozytensors")
            writer.component("net").put(
                "weight", torch.randn((128, 128), generator=generator) * 0.01
            )
            checkpoint = writer.commit()
            checkpoints.append(checkpoint)
            with torch.device("meta"):
                network = nn.Linear(128, 128, bias=False)
            vars(network)["_cozy_weights"] = residency.Declared(
                residency.Weights(checkpoint, "net"), blocks=[]
            )
            models[name] = memory.load(network, "ram")

        def prepare(name, steps):
            model = models[name]
            state = torch.ones((8, 128), device="cuda") * 0.01
            binding = model.retain()

            def step(index):
                state.copy_(torch.tanh(model.module(state) + 0.001))

            try:
                with (
                    memory.request(f"capture-{name}"),
                    binding.prepare(),
                    torch.no_grad(),
                ):
                    plan = PreparedRepeat.capture(
                        step, state=(state,), steps=steps, owners=(binding,)
                    )
            finally:
                binding.close()
            plans.append(plan)
            return plan

        try:
            unrelated = prepare("unrelated", 3)
            unrelated_result = unrelated.submit().wait()[0]
            for index in range(4):
                plan = prepare("active", 10_000_000)
                execution = plan.submit()
                assert models["active"].users == 1
                assert not memory._retire(models["active"])
                ready = threading.Barrier(3)

                def wait(observed, barrier):
                    barrier.wait(timeout=10)
                    return observed.wait()[0]

                with ThreadPoolExecutor(max_workers=2) as pool:
                    first = pool.submit(wait, execution, ready)
                    second = pool.submit(wait, execution, ready)
                    ready.wait(timeout=10)
                    # Observers park while the GPU continues independently.
                    time.sleep(0.02)
                    started = time.monotonic()
                    (execution.close if index % 2 else plan.close)()
                    first_output = first.result(timeout=30)
                    second_output = second.result(timeout=30)
                elapsed_ms = (time.monotonic() - started) * 1000
                progress = execution.poll()
                assert progress.done and progress.canceled
                assert 0 < progress.completed_steps < plan.steps
                assert models["active"].users == 0
                assert first_output is second_output
                assert torch.isfinite(first_output).all().item()
                assert plan.facts()["closed"]

                # Measured managed headroom makes this a real size-targeted reclaim request.
                # It runs after cooperative completion, never in an allocator/Python callback.
                before = int(memory.native.stats()["resident_bytes"])
                wanted = memory.room().free + memory.granularity
                released = memory.reclaim(wanted, current={models["unrelated"].name})
                after = int(memory.native.stats()["resident_bytes"])
                assert before - after >= memory.granularity, (before, after, released)
                assert not unrelated.facts()["closed"]
                rounds.append(
                    {
                        "completed_steps": progress.completed_steps,
                        "cancel_join_ms": elapsed_ms,
                        "native_reclaimed_bytes": before - after,
                        "reported_released_bytes": released,
                    }
                )

            unrelated.state[0].fill_(0.01)
            torch.testing.assert_close(unrelated_result, unrelated.submit().wait()[0])
            print(
                json.dumps(
                    {
                        "result": {
                            "passed": True,
                            "rounds": rounds,
                            "single_readout_owner": True,
                            "balanced_model_claims": True,
                            "active_retirement_refused": True,
                            "unrelated_definition_preserved": True,
                            "managed_budget_bytes": 256 << 20,
                        }
                    }
                ),
                flush=True,
            )
        finally:
            for plan in plans:
                plan.close()
            for model in models.values():
                model.close()
            memory.trim()
            for checkpoint in checkpoints:
                checkpoint.close()


if __name__ == "__main__":
    main()
