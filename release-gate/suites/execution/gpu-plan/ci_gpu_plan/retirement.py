"""Real model-local offload and changed-fill retirement of cached native definitions."""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

import tensorfs
import torch
from cozy_runtime.compute import PreparedRepeat
from cozy_runtime.internal import accel, residency
from torch import nn


class ScaledFill:
    def __init__(self):
        self.scale = 1

    @property
    def identity(self):
        return f"retirement-fill-{self.scale}"

    def __call__(self, block, parts):
        for roles in parts.values():
            roles["value"].mul_(self.scale)


def main():
    seed = int(sys.argv[1])
    torch.set_num_threads(1)
    accel.initialize(torch, "cuda")
    memory = residency.Memory.open("cuda:0")
    generator = torch.Generator().manual_seed(seed)
    models, networks, checkpoints, plans = {}, {}, [], []
    fill = ScaledFill()
    initial = torch.randn((8, 128), generator=generator).cuda() * 0.01
    with tempfile.TemporaryDirectory(prefix="cozy-plan-retire-") as directory:
        for name in ("a", "b"):
            writer = tensorfs.Writer(Path(directory) / f"{name}.cozytensors")
            writer.component("net").put(
                "weight", torch.randn((128, 128), generator=generator) * 0.01
            )
            checkpoint = writer.commit()
            checkpoints.append(checkpoint)
            with torch.device("meta"):
                network = nn.Linear(128, 128, bias=False)
            vars(network)["_cozy_weights"] = residency.Declared(
                residency.Weights(checkpoint, "net"), blocks=[], fill=fill if name == "a" else None
            )
            networks[name] = network
            models[name] = memory.load(network, "ram")

        def prepare(name, steps=3):
            state = initial.clone()
            network = networks[name]
            binding = models[name].retain()

            def step(index):
                state.add_(network(state) * 0.01)

            try:
                with memory.request(f"capture-{name}"), binding.prepare(), torch.no_grad():
                    plan = PreparedRepeat.capture(
                        step,
                        state=(state,),
                        steps=steps,
                        owners=(binding, network, checkpoints[0 if name == "a" else 1]),
                    )
            finally:
                binding.close()  # native definition must own a distinct retained binding
            plans.append(plan)
            return plan, state

        try:
            a, _a_state = prepare("a")
            b, b_state = prepare("b")
            first_a = a.submit().wait()[0]
            expected_b = b.submit().wait()[0]
            assert models["a"].users == models["b"].users == 0

            memory.load(networks["a"], "ram")
            assert a.facts()["closed"] and not b.facts()["closed"]
            reclaimed = memory.native.reclaim(memory.granularity)
            assert reclaimed >= memory.granularity, (reclaimed, memory.native.stats())
            try:
                a.submit()
            except RuntimeError:
                pass
            else:
                raise AssertionError("offloaded definition remained usable")

            a2, _ = prepare("a")
            torch.testing.assert_close(a2.submit().wait()[0], first_a, rtol=1e-5, atol=1e-6)
            fill.scale = 2
            a3, _ = prepare("a")  # retain -> _declare must retire stale a2 automatically
            assert a2.facts()["closed"] and not b.facts()["closed"]
            changed = a3.submit().wait()[0]
            assert not torch.equal(changed, first_a)

            active_plan, _ = prepare("a", 100000)
            active = active_plan.submit()
            assert models["a"].users > 0
            try:
                memory.load(networks["a"], "ram")
            except residency.ResidencyError:
                pass
            else:
                raise AssertionError("active model was offloaded")
            assert not active_plan.facts()["closed"] and not b.facts()["closed"]
            active.cancel()
            active.wait()
            assert models["a"].users == 0
            memory.load(networks["a"], "ram")
            assert active_plan.facts()["closed"] and a3.facts()["closed"]
            assert not b.facts()["closed"]
            b_state.copy_(initial)
            torch.testing.assert_close(b.submit().wait()[0], expected_b, rtol=1e-5, atol=1e-6)
            print(
                json.dumps(
                    {
                        "result": {
                            "passed": True,
                            "offload_reclaimed_bytes": reclaimed,
                            "old_definition_closed": True,
                            "recapture_matched": True,
                            "fill_identity_retired": True,
                            "active_offload_refused": True,
                            "unrelated_model_preserved": True,
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
