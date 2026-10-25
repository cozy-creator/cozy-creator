"""Real stored overlay reads: independent CUDA math, ownership and Python work counts."""

from __future__ import annotations

import cProfile
import gc
import importlib.metadata
import pstats
import tempfile
import time
from pathlib import Path

import tensorfs
import torch
from cozy_runtime.author.kernels import readable
from cozy_runtime.internal import residency
from torch import nn

PAIRS = 1182


class Factors(nn.Module):
    def __init__(self):
        super().__init__()
        self.down = nn.Linear(32, 4, bias=False)
        self.up = nn.Linear(4, 32, bias=False)
        self.tied = self.down.weight


def network(checkpoint, count, *, fill=None):
    with torch.device("meta"):
        module = nn.ModuleList(Factors() for _ in range(count))
    keys = tuple(f"{i}.{side}.weight" for i in range(count) for side in ("down", "up"))
    vars(module)["_cozy_weights"] = residency.Declared(
        residency.Weights(checkpoint, "overlay", keys=keys), blocks=[], fill=fill
    )
    return module


def calls(profile):
    return {
        name: sum(
            value[1]
            for (filename, _, function), value in pstats.Stats(profile).stats.items()
            if filename.endswith("/residency.py") and function == name
        )
        for name in ("_read", "_ensure", "_assign")
    }


def exercise(seed, ctx):
    assert ctx.device.type == "cuda"
    memory = residency.Memory.open("cuda:0")
    assert memory.native is not None
    generator = torch.Generator().manual_seed(seed)
    down = torch.randint(-16, 17, (4, 32), generator=generator).float() / 32
    up = torch.randint(-16, 17, (32, 4), generator=generator).float() / 32
    inputs = torch.arange(64, dtype=torch.float32).reshape(2, 32).remainder(7) / 8
    reference = (inputs @ down.T) @ up.T
    gpu_input = inputs.cuda()
    output = torch.zeros_like(gpu_input)
    owners = []
    with tempfile.TemporaryDirectory(prefix="cozy-factor-reads-") as directory:
        writer = tensorfs.Writer(Path(directory) / "overlay.cozytensors")
        for i in range(PAIRS):
            writer.component("overlay").put(f"{i}.down.weight", down)
            writer.component("overlay").put(f"{i}.up.weight", up)
        checkpoint = writer.commit()
        try:
            module = network(checkpoint, PAIRS)
            owner = memory.load(module, "ram")
            owners.append(owner)
            with memory.request("factor-reads:cold"), torch.no_grad():
                with readable(module[0].down.weight, module[0].up.weight) as factors:
                    cold = (gpu_input @ factors[0].T) @ factors[1].T
                del factors
                torch.testing.assert_close(cold.cpu(), reference, rtol=0, atol=0)
            block = owner.common
            assert block is not None and block.region is not None
            assert (
                block.region.resident and block.data is None and block.assigned is None
            )
            placeholders = tuple(id(value) for value in module.parameters())
            profile = cProfile.Profile()
            torch.cuda.synchronize()
            began = time.perf_counter()
            with memory.request("factor-reads:ready") as request, torch.no_grad():
                profile.enable()
                try:
                    for i in range(30):
                        part = module[(i * 37) % PAIRS]
                        with readable(part.down.weight, part.up.weight) as factors:
                            output.add_((gpu_input @ factors[0].T) @ factors[1].T)
                        del factors
                finally:
                    profile.disable()
                torch.cuda.synchronize()
            elapsed_ms = (time.perf_counter() - began) * 1000
            measured = calls(profile)
            assert measured == {"_read": 60, "_ensure": 0, "_assign": 0}, measured
            assert request.stats()["moved"]["store"]["bytes"] == 0
            assert tuple(id(value) for value in module.parameters()) == placeholders
            assert block.data is None and block.assigned is None and block.count == 0
            torch.testing.assert_close(output.cpu(), reference * 30, rtol=0, atol=0)

            original = module[0].down.weight
            with owner.passing("active-tied"), readable(original) as factors:
                assert factors[0] is module[0].down.weight is module[0].tied
                assert isinstance(factors[0], nn.Parameter)
            del factors
            converted = original.to(dtype=torch.bfloat16)
            with readable(converted) as factors:
                assert factors[0].dtype is torch.bfloat16
                torch.testing.assert_close(
                    factors[0].cpu(), down.bfloat16(), rtol=0, atol=0
                )
            del factors, converted

            with readable(original) as factors:
                escaped = factors[0][:2]
            del factors
            memory.native.reclaim(1 << 30)
            assert block.region.resident
            torch.testing.assert_close(escaped.cpu(), down[:2], rtol=0, atol=0)
            del escaped
            gc.collect()
            memory.native.reclaim(1 << 30)
            assert not block.region.resident
            profile = cProfile.Profile()
            with memory.request("factor-reads:refill") as refill, torch.no_grad():
                profile.enable()
                try:
                    with readable(original) as factors:
                        torch.testing.assert_close(
                            factors[0].cpu(), down, rtol=0, atol=0
                        )
                    del factors
                finally:
                    profile.disable()
            cold_counts = calls(profile)
            assert cold_counts["_ensure"] == 1 and cold_counts["_assign"] == 1, (
                cold_counts
            )
            assert refill.stats()["moved"]["store"]["bytes"] > 0

            stream = torch.cuda.Stream()
            with torch.cuda.stream(stream), readable(original) as factors:
                consumer = gpu_input @ factors[0].T
            del factors
            # Native reclamation must join consumers before reusing the released backing.
            memory.native.reclaim(1 << 30)
            torch.testing.assert_close(consumer.cpu(), inputs @ down.T, rtol=0, atol=0)

            class Scale:
                value = 1

                @property
                def identity(self):
                    return f"factor-scale:{self.value}"

                def __call__(self, block, parts):
                    del block
                    for row in parts.values():
                        row["value"].mul_(self.value)

            scale = Scale()
            changing = network(checkpoint, 1, fill=scale)
            changed_owner = memory.load(changing, "ram")
            owners.append(changed_owner)
            with readable(changing[0].down.weight) as factors:
                old_fill = factors[0][:2]
            del factors
            scale.value = 2
            with readable(changing[0].down.weight) as factors:
                torch.testing.assert_close(factors[0].cpu(), down * 2, rtol=0, atol=0)
            del factors
            torch.testing.assert_close(old_fill.cpu(), down[:2], rtol=0, atol=0)
            del old_fill
            for held in owners:
                held.close()
            owners.clear()
            gc.collect()
            memory.native.reclaim(1 << 30)
            cleanup = memory.native.stats()
            assert cleanup["pinned_bytes"] == 0 and not cleanup["poisoned"], cleanup
            return {
                "passed": True,
                "runtime": importlib.metadata.version("cozy-runtime"),
                "tensorfs": tensorfs.__version__,
                "gpu": torch.cuda.get_device_name(),
                "stored_factors": PAIRS * 2,
                "ready_reads": measured,
                "ready_profiled_wall_ms": elapsed_ms,
                "refill_calls": cold_counts,
                "zero_ready_read_bytes": True,
                "tied_active_parameter": True,
                "dtype_conversion": "BF16",
                "escaped_alias_and_stream_completion": True,
                "changed_fill_kept_old_alias": True,
                "cleanup": cleanup,
                "scope": "Real component correctness and cProfile call counts; not H3 speedup or throughput",
            }
        finally:
            for held in owners:
                held.close()
            checkpoint.close()
