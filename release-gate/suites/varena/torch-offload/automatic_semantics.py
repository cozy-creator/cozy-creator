"""Actual public automatic-mode alias/container/RNG exclusions, no fake graphs.

CPU exercises real eager wrappers only; operator GPU mode checks controller
selection counters and the original callable's exact return/RNG semantics.
"""

from __future__ import annotations

import argparse
import json
from contextlib import nullcontext

import torch
from tensorfs.varena import torch as vt


class ContainerModule(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.weight = torch.nn.Parameter(torch.zeros(4))
        self.calls = 0

    def forward(self, value):
        self.calls += 1
        result = value + self.weight
        return {"duplicate": (result, result), "view": result.view(-1), "none": None}


class RandomModule(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.weight = torch.nn.Parameter(torch.zeros(4))
        self.generator = None
        self.calls = 0

    def forward(self, value):
        self.calls += 1
        return (
            value
            + self.weight
            + torch.rand(
                value.shape,
                dtype=value.dtype,
                device=value.device,
                generator=self.generator,
            )
        )


def check_facts(owner, device, reason, requests, *, observed_cold):
    facts = owner.graph_facts()
    assert len(facts["boundaries"]) == 1
    row = next(iter(facts["boundaries"].values()))
    assert row["eager_calls"] == requests and row["captures"] == row["replays"] == 0
    assert row["cache"] is None
    if device.type == "cuda":
        assert row["reason"] == reason, row
        assert row["cold_observations"] == observed_cold
    else:
        assert row["reason"] == "cpu" and row["cold_observations"] == 0
    return facts


def run(args):
    device = torch.device(args.device)
    rows = {}

    # Open first owner BEFORE any CUDA tensor/generator is made.
    identity = torch.nn.Identity().eval()
    owner = vt.offload(identity, device=device, graphs="auto")
    stream = torch.cuda.Stream(device=device) if device.type == "cuda" else None
    scope = torch.cuda.stream(stream) if stream is not None else nullcontext()
    with torch.no_grad(), scope:
        value = torch.arange(8.0, device=device).reshape(2, 4)
        for _ in range(3):
            assert identity(value) is value
        rows["identity"] = check_facts(
            owner, device, "output_aliases_input", 3, observed_cold=1
        )
    owner.close()

    model = ContainerModule().eval()
    owner = vt.offload(model, device=device, graphs="auto")
    scope = torch.cuda.stream(stream) if stream is not None else nullcontext()
    with torch.no_grad(), scope:
        for _ in range(3):
            result = model(value)
            assert type(result) is dict and result["none"] is None
            assert type(result["duplicate"]) is tuple
            assert result["duplicate"][0] is result["duplicate"][1]
            assert torch._C._is_alias_of(result["duplicate"][0], result["view"])
            torch.testing.assert_close(result["view"], value.flatten(), rtol=0, atol=0)
        assert model.calls == 3
        rows["container"] = check_facts(
            owner, device, "unreviewed_module_effects", 3, observed_cold=0
        )
    owner.close()

    random = RandomModule().eval()
    owner = vt.offload(random, device=device, graphs="auto")
    # Two independent REAL generators. No save/restore, reseed or rewind masks
    # extra module execution: each must advance exactly once per request.
    random.generator = torch.Generator(device=device).manual_seed(731)
    control = torch.Generator(device=device).manual_seed(731)
    scope = torch.cuda.stream(stream) if stream is not None else nullcontext()
    with torch.no_grad(), scope:
        for _ in range(3):
            expected = value + torch.rand(value.shape, device=device, generator=control)
            actual = random(value)
            torch.testing.assert_close(actual, expected, rtol=0, atol=0)
            assert torch.equal(random.generator.get_state(), control.get_state())
        assert random.calls == 3
        rows["rng"] = check_facts(
            owner, device, "unreviewed_module_effects", 3, observed_cold=0
        )
    owner.close()

    inplace = torch.nn.ReLU(inplace=True).eval()
    owner = vt.offload(inplace, device=device, graphs="auto")
    scope = torch.cuda.stream(stream) if stream is not None else nullcontext()
    with torch.no_grad(), scope:
        for _ in range(3):
            argument = torch.tensor([-2.0, -1.0, 1.0, 2.0], device=device)
            result = inplace(argument)
            assert result is argument
            torch.testing.assert_close(
                result,
                torch.tensor([0.0, 0.0, 1.0, 2.0], device=device),
                rtol=0,
                atol=0,
            )
        rows["inplace"] = check_facts(
            owner, device, "inplace_module", 3, observed_cold=0
        )
        if stream is not None:
            stream.synchronize()
    owner.close()
    if device.type == "cpu":
        assert not torch.cuda.is_initialized()
    return {
        "passed": True,
        "device": str(device),
        "scope": "actual public eager callable semantics; GPU selection evidence only when device=cuda",
        "cases": rows,
        "alias_none_container_semantics_preserved": True,
        "rng_generator_advanced_once_per_request": True,
        "gpu_executed": device.type == "cuda",
        "cuda_initialized": torch.cuda.is_initialized(),
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", default="cuda:0")
    options = parser.parse_args()
    torch.set_num_threads(1)
    torch.set_num_interop_threads(1)
    print(json.dumps(run(options), indent=2))
