"""Default-CLI candidate measurement of real Runtime derived weight bindings.

Uses actual TensorFS files, CUDA tensors, and residency hooks. Held results are an explicit
pinning baseline, not an evictable-cache proposal. This is not whole-model performance proof.
"""
from __future__ import annotations

import hashlib
import json
import math
import sys
import tempfile
import time
from contextlib import nullcontext
from pathlib import Path

import tensorfs
import torch
from torch import nn

from cozy_runtime.internal import accel, residency


class Binder:
    def __init__(self, kind: str) -> None:
        self.kind = kind
        self.calls = 0

    def __call__(self, key, parts):
        self.calls += 1
        source = parts["value"]
        if self.kind == "fp8_decode":
            return {key: source.float()}
        if self.kind == "derived_transpose":
            return {key: source.t().contiguous()}
        return {key: source}


def exercise(device: str, seed: int, repeats: int = 16) -> dict:
    torch.set_num_threads(1)
    accel.initialize(torch, "cuda")
    memory = residency.Memory.open(device)
    generator = torch.Generator().manual_seed(seed)
    width = 1024
    inputs_cpu = torch.randn(4, width, generator=generator)
    inputs = inputs_cpu.to(device)
    result = {
        "device": device,
        "seed": seed,
        "repeats": repeats,
        "width": width,
        "torch": torch.__version__,
        "tensorfs": tensorfs.__version__,
        "residency_sha256": hashlib.sha256(Path(residency.__file__).read_bytes()).hexdigest(),
        "scope": "single-block real FP8 decode / derived transpose; not full-model performance",
        "cases": {},
    }
    with tempfile.TemporaryDirectory(prefix="cozy-runtime-derived-") as directory:
        for kind in ("raw_f32", "fp8_decode", "derived_transpose"):
            original = torch.randn(width, width, generator=generator) / math.sqrt(width)
            stored = original.to(torch.float8_e4m3fn) if kind == "fp8_decode" else original
            target = stored.float().t() if kind == "derived_transpose" else stored.float()
            expected = torch.nn.functional.linear(inputs_cpu, target)
            writer = tensorfs.Writer(Path(directory) / f"{kind}.cozytensors")
            writer.component("net").put("0.weight", stored)
            checkpoint = writer.commit()
            binder = Binder(kind)
            with torch.device("meta"):
                network = nn.Sequential(nn.Linear(width, width, bias=False))
            vars(network)["_cozy_weights"] = residency.Declared(
                residency.Weights(checkpoint, "net"), blocks=["0"], bind=binder
            )
            owner = memory.load(network, "ram")
            with memory.request(f"{kind}:cold") as cold, torch.no_grad():
                first = network(inputs).cpu()
            torch.testing.assert_close(first, expected, rtol=2e-4, atol=2e-5)
            case = {
                "stored_bytes": stored.nbytes,
                "derived_bytes": target.nbytes if kind != "raw_f32" else 0,
                "cold_store_bytes": cold.stats()["moved"]["store"]["bytes"],
                "native_after_cold": memory.native.stats(),
                "phases": {},
            }
            for pinned in (False, True):
                name = "held_baseline" if pinned else "idle_native_cache"
                hold = owner.hold() if pinned else nullcontext()
                assert hold is not None
                with hold, torch.no_grad():
                    torch.cuda.synchronize()
                    before_calls = binder.calls
                    begin = torch.cuda.Event(enable_timing=True)
                    end = torch.cuda.Event(enable_timing=True)
                    began = time.perf_counter_ns()
                    begin.record()
                    with memory.request(f"{kind}:{name}") as request:
                        for _ in range(repeats):
                            output = network(inputs)
                    end.record()
                    end.synchronize()
                    wall_ms = (time.perf_counter_ns() - began) / 1e6
                    phase = {
                        "bind_calls": binder.calls - before_calls,
                        "store_bytes": request.stats()["moved"]["store"]["bytes"],
                        "wall_ms_per_call": wall_ms / repeats,
                        "cuda_ms_per_call": begin.elapsed_time(end) / repeats,
                        "native": memory.native.stats(),
                    }
                    torch.testing.assert_close(output.cpu(), expected, rtol=2e-4, atol=2e-5)
                    assert phase["store_bytes"] == 0
                    assert phase["bind_calls"] == (0 if pinned else repeats)
                    case["phases"][name] = phase
                assert all(block.data is None for block in owner.groups())
            result["cases"][kind] = case
            owner.close()
            del owner, network, output, first
            checkpoint.close()
            memory.trim()
    result["passed"] = True
    return result


if __name__ == "__main__":
    print(json.dumps({"result": exercise(sys.argv[1], int(sys.argv[2]))}), flush=True)
