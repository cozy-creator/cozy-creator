"""Actual file-backed CUDA weights and immutable final-native-buffer qualification."""

from __future__ import annotations

import hashlib
import importlib.metadata
import json
import math
import sys
import tempfile
import time
from pathlib import Path

import numpy as np
import tensorfs
import torch
from cozy_runtime import _build_provenance
from cozy_runtime.internal import accel, residency
from tensorfs.raw import decode_into
from tensorfs.raw import parts as stored_parts
from torch import nn

MIB = 1 << 20


class FinalWriter:
    def __init__(self, checkpoint, width: int, kind: str, dtype: str = "F32") -> None:
        self.width, self.kind, self.dtype = width, kind, dtype
        self.metadata = stored_parts(checkpoint, "net", "0.weight")
        self.identity = f"native-refill:{kind}:1:{dtype}:{width}x{width}"
        self.calls = 0

    def layout(self, key):
        return {key: residency.TensorLayout(self.dtype, (self.width, self.width))}

    def write(self, key, parts, destinations):
        self.calls += 1
        out = destinations[key]
        if self.kind == "fp8.tensorwise":
            decode_into(
                self.kind,
                {
                    role: (part.dtype, part.shape, parts[role])
                    for role, part in self.metadata.items()
                },
                out,
                dtype=self.dtype,
                shape=(self.width, self.width),
            )
        elif self.kind == "transpose":
            out.copy_(parts["value"].t())
        else:
            out.copy_(parts["value"])

    def __call__(self, key, parts):
        out = torch.empty(
            (self.width, self.width),
            dtype=getattr(torch, residency._DTYPES[self.dtype]),
            device="cuda",
        )
        self.write(key, parts, {key: out})
        return {key: out}


def network(
    checkpoint, width, layers=1, *, fill=None, finals=None, dtype=torch.float32
):
    with torch.device("meta"):
        module = nn.Sequential(
            *(nn.Linear(width, width, bias=False, dtype=dtype) for _ in range(layers))
        )
    vars(module)["_cozy_weights"] = residency.Declared(
        residency.Weights(checkpoint, "net"),
        blocks=[str(i) for i in range(layers)],
        fill=fill,
        bind=finals,
        finals=finals,
    )
    return module


def write_matrix(directory, kind, width, generator):
    weight = torch.randn(width, width, generator=generator) / math.sqrt(width)
    writer = tensorfs.Writer(Path(directory) / f"{kind}.cozytensors")
    component = writer.component("net")
    if kind == "fp8.tensorwise":
        scale = np.array(0.125, dtype=np.float32)
        payload = (weight / float(scale)).to(torch.float8_e4m3fn)
        component.put_encoded(
            "0.weight",
            dtype="F32",
            shape=(width, width),
            scheme=kind,
            params={},
            parts={
                "weight": (
                    "F8_E4M3",
                    (width, width),
                    payload.view(torch.uint8).numpy(),
                ),
                "scale": ("F32", (), scale),
            },
        )
        expected = payload.float() * float(scale)
    else:
        component.put("0.weight", weight.double() if kind == "cast" else weight)
        expected = weight.t() if kind == "transpose" else weight
    return writer.commit(), expected


def production_routes(memory, seed):
    from types import SimpleNamespace

    from cozy_runtime.author._loader import census
    from cozy_runtime.internal.encoding import launch_providers
    from cozy_runtime.internal.encoding.selection import measure_device, measure_runtime
    from cozy_runtime.internal.fill import Checkpoint, dtype_name
    from cozy_runtime.internal.resolution import ConstructionFacts, Variant, resolve
    from cozy_runtime.internal.weights import Backend

    generator = torch.Generator().manual_seed(seed)
    facts = measure_device(torch, 0, "cuda")
    result = {}
    with tempfile.TemporaryDirectory(prefix="cozy-production-routes-") as directory:
        for kind in ("raw-f16", "fp8-decoded-f16", "fp8-native-f16"):
            width = 512
            weight = torch.randn(width, width, generator=generator) / width**0.5
            writer = tensorfs.Writer(Path(directory) / f"{kind}.cozytensors")
            component = writer.component("net")
            if kind == "raw-f16":
                expected = weight.half()
                component.put("0.weight", expected)
            else:
                payload = (weight / 0.125).to(torch.float8_e4m3fn)
                component.put_encoded(
                    "0.weight",
                    dtype="F16",
                    shape=(width, width),
                    scheme="fp8.tensorwise",
                    params={},
                    parts={
                        "weight": (
                            "F8_E4M3",
                            (width, width),
                            payload.view(torch.uint8).numpy(),
                        ),
                        "scale": ("F32", (), np.array(0.125, dtype=np.float32)),
                    },
                )
                expected = (payload.float() * 0.125).half()
            checkpoint = writer.commit()
            opened = Checkpoint(checkpoint)
            rows = opened.rows("net")
            consent = "accept" if kind == "fp8-native-f16" else "refuse"
            plan = resolve(
                variant=Variant(
                    kind, str(directory), checkpoint.id, {"net": checkpoint.id}
                ),
                rows=rows,
                providers=launch_providers(),
                device=facts,
                runtime=measure_runtime(
                    torch, importlib.metadata.version("cozy-runtime")
                ),
                construction=ConstructionFacts(
                    "native-refill-proof", "RouteModel", ("net",)
                ),
                encoded_leaves=consent,
                dtype_name=dtype_name,
            )
            with torch.device("meta"):
                module = nn.Sequential(
                    nn.Linear(width, width, bias=False, dtype=torch.float16)
                )
            built = SimpleNamespace(net=module)
            walked = census(built)
            backend = Backend(
                {"net": opened},
                rows,
                torch=torch,
                plan=plan,
                memory=memory,
                device=torch.device("cuda:0"),
                construction="RouteModel",
                device_facts=facts,
                release="native-refill-proof",
            )
            backend.expect({"net": [row.key for row in rows]})
            fit = backend.fit(walked, encoded_leaves=consent)
            assert fit and fit["ok"]
            live = backend.materialize(built, walked, encoded_leaves=consent)
            for destination in walked.destinations:
                backend.fill(destination.key, destination.spec, live[destination.key])
            backend.commit()
            inputs = torch.randn(4, width, generator=generator).half().to("cuda")
            oracle = torch.nn.functional.linear(inputs, expected.to("cuda"))
            try:
                with memory.request(f"routes:{kind}:cold") as cold, torch.no_grad():
                    actual = module(inputs).clone()
                torch.testing.assert_close(actual, oracle, rtol=2e-2, atol=2e-2)
                with memory.request(f"routes:{kind}:warm") as warm, torch.no_grad():
                    for _ in range(8):
                        again = module(inputs)
                torch.testing.assert_close(again, actual, rtol=0, atol=0)
                assert warm.stats()["moved"]["store"]["bytes"] == 0
                assert warm.stats()["final_bindings"]["fills"] == 0
                assert memory.native.stats()["pinned_bytes"] == 0
                dtype_transition = None
                if kind != "fp8-native-f16":
                    owner = backend.models["net"]
                    binding = owner.retain()
                    with binding.prepare(), torch.no_grad():
                        pass
                    module.to(dtype=torch.float32)
                    assert not binding.valid()
                    try:
                        with binding.prepare():
                            pass
                    except residency.StaleBinding:
                        pass
                    else:
                        raise AssertionError(
                            "graph binding accepted a changed target dtype"
                        )
                    binding.close()
                    with (
                        memory.request(f"routes:{kind}:wide") as widened,
                        torch.no_grad(),
                    ):
                        wide = module(inputs.float())
                    torch.testing.assert_close(
                        wide,
                        torch.nn.functional.linear(
                            inputs.float(), expected.float().to("cuda")
                        ),
                        rtol=2e-4,
                        atol=2e-5,
                    )
                    module.to(dtype=torch.float16)
                    with (
                        memory.request(f"routes:{kind}:restored") as restored,
                        torch.no_grad(),
                    ):
                        original = module(inputs)
                    torch.testing.assert_close(original, actual, rtol=0, atol=0)
                    assert widened.stats()["moved"]["store"]["bytes"] == 0
                    assert restored.stats()["moved"]["store"]["bytes"] == 0
                    dtype_transition = {
                        "binding_refused": True,
                        "wide": widened.stats(),
                        "restored": restored.stats(),
                    }
                result[kind] = {
                    "dtype_transition": dtype_transition,
                    "routes": plan.route_census(),
                    "label": plan.label(),
                    "plan": plan.digest(),
                    "cold": cold.stats(),
                    "warm": warm.stats(),
                }
            finally:
                backend.close()
                memory.trim()
                checkpoint.close()
    return result


def fits(memory, seed):
    generator = torch.Generator().manual_seed(seed)
    width, repeats = 512, 16
    result = {}
    with tempfile.TemporaryDirectory(prefix="cozy-final-fits-") as directory:
        for kind in ("raw", "cast", "fp8.tensorwise", "transpose"):
            checkpoint, weight = write_matrix(directory, kind, width, generator)
            writer = (
                FinalWriter(checkpoint, width, kind)
                if kind in {"fp8.tensorwise", "transpose"}
                else None
            )
            module = network(checkpoint, width, finals=writer)
            owner = memory.load(module, "ram")
            inputs_cpu = torch.randn(4, width, generator=generator)
            inputs = inputs_cpu.to("cuda")
            expected = torch.nn.functional.linear(inputs_cpu, weight)
            with memory.request(f"{kind}:cold") as cold, torch.no_grad():
                first = module(inputs).cpu()
            torch.testing.assert_close(first, expected, rtol=2e-4, atol=2e-5)
            block = owner.groups()[0]
            cold_writer_calls = 0 if writer is None else writer.calls
            released_raw = 0
            if block.final_only():
                assert block.region is not None and not block.region.preferred
                released_raw = memory.native.reclaim(int(block.region.allocated_bytes))
                assert not block.region.resident and block.final_region.resident
            begin, end = (
                torch.cuda.Event(enable_timing=True),
                torch.cuda.Event(enable_timing=True),
            )
            torch.cuda.synchronize()
            began = time.perf_counter_ns()
            begin.record()
            with memory.request(f"{kind}:warm") as warm, torch.no_grad():
                for _ in range(repeats):
                    output = module(inputs)
            end.record()
            end.synchronize()
            wall_ms = (time.perf_counter_ns() - began) / 1e6
            torch.testing.assert_close(output.cpu(), first, rtol=0, atol=0)
            warm_stats = warm.stats()
            assert warm_stats["moved"]["store"]["bytes"] == 0
            assert warm_stats["final_bindings"]["fills"] == 0
            assert writer is None or writer.calls == cold_writer_calls
            assert all(
                b.data is None and b.final_data is None and b.final_use is None
                for b in owner.groups()
            )
            assert memory.native.stats()["pinned_bytes"] == 0
            result[kind] = {
                "cold": cold.stats(),
                "warm": warm_stats,
                "cold_writer_calls": cold_writer_calls,
                "warm_writer_calls": 0
                if writer is None
                else writer.calls - cold_writer_calls,
                "raw_reclaimed_before_warm": released_raw,
                "wall_ms_per_call": wall_ms / repeats,
                "cuda_ms_per_call": begin.elapsed_time(end) / repeats,
                "metadata": {
                    role: {
                        "dtype": part.dtype,
                        "shape": part.shape,
                        "bytes": part.length,
                    }
                    for role, part in stored_parts(
                        checkpoint, "net", "0.weight"
                    ).items()
                },
                "decoder_scratch_bound": None if kind == "fp8.tensorwise" else 0,
            }
            print(json.dumps({"case": kind, "evidence": result[kind]}), flush=True)
            owner.close()
            del owner, module, inputs, output
            memory.trim()
            checkpoint.close()
    return result


def streaming(memory, seed):
    generator = torch.Generator().manual_seed(seed)
    width, layers = 1024, 24
    with tempfile.TemporaryDirectory(prefix="cozy-final-stream-") as directory:
        writer = tensorfs.Writer(Path(directory) / "stream.cozytensors")
        component = writer.component("net")
        weights = [
            torch.randn(width, width, generator=generator) / math.sqrt(width)
            for _ in range(layers)
        ]
        for index, weight in enumerate(weights):
            component.put(f"{index}.weight", weight.double())
        checkpoint = writer.commit()
        module = network(checkpoint, width, layers)
        owner = memory.load(module, "ram")
        inputs_cpu = torch.randn(2, width, generator=generator)
        expected = inputs_cpu
        for weight in weights:
            expected = torch.nn.functional.linear(expected, weight)
        inputs = inputs_cpu.to("cuda")
        passes = []
        for index in range(3):
            with memory.request(f"stream:{index}") as request, torch.no_grad():
                actual = module(inputs).cpu()
            torch.testing.assert_close(actual, expected, rtol=3e-4, atol=2e-5)
            passes.append(request.stats())
            assert memory.native.stats()["pinned_bytes"] == 0
        assert passes[1]["final_bindings"]["fills"] < layers
        assert (
            passes[2]["moved"]["store"]["bytes"] == passes[1]["moved"]["store"]["bytes"]
        )
        assert (
            passes[2]["final_bindings"]["final_resident_bytes"]
            > passes[2]["final_bindings"]["raw_resident_bytes"]
        )
        owner.close()
        memory.trim()
        checkpoint.close()
        return {
            "stored_bytes": width * width * 8 * layers,
            "final_bytes": width * width * 4 * layers,
            "passes": passes,
        }


def failed_fill(memory, seed):
    generator = torch.Generator().manual_seed(seed)
    with tempfile.TemporaryDirectory(prefix="cozy-final-failure-") as directory:
        checkpoint, weight = write_matrix(directory, "transpose", 32, generator)

        class FailOnce(FinalWriter):
            escaped = None
            failed = False

            def write(self, key, parts, destinations):
                if not self.failed:
                    self.failed = True
                    target = destinations[key]
                    target.fill_(7)
                    self.escaped = target[:2]
                    raise RuntimeError(
                        "deliberate failure after writing a real native destination"
                    )
                super().write(key, parts, destinations)

        writer = FailOnce(checkpoint, 32, "transpose")
        module = network(checkpoint, 32, finals=writer)
        owner = memory.load(module, "ram")
        inputs = torch.ones(2, 32, device="cuda")
        try:
            with memory.request("failed-fill"), torch.no_grad():
                module(inputs)
        except RuntimeError as error:
            assert str(error).startswith("deliberate failure")
        else:
            raise AssertionError("the cold writer did not fail")
        block = owner.groups()[0]
        assert (
            block.count == 0 and block.final_region is None and block.final_data is None
        )
        escaped = writer.escaped
        assert escaped is not None
        with memory.request("after-failure") as request, torch.no_grad():
            actual = module(inputs).cpu()
        torch.testing.assert_close(
            actual,
            torch.nn.functional.linear(inputs.cpu(), weight),
            rtol=2e-4,
            atol=2e-5,
        )
        memory.trim()
        torch.testing.assert_close(
            escaped.cpu(), torch.full((2, 32), 7.0), rtol=0, atol=0
        )
        assert memory.native.stats()["pinned_bytes"] > 0
        writer.escaped = None
        del escaped
        owner.close()
        memory.trim()
        checkpoint.close()
        assert memory.native.stats()["pinned_bytes"] == 0
        return {
            "after_failure": request.stats(),
            "failed_alias_preserved": True,
            "native_after_close": memory.native.stats(),
        }


def lora(memory, seed):
    generator = torch.Generator().manual_seed(seed)
    width, rank = 64, 4
    with tempfile.TemporaryDirectory(prefix="cozy-final-lora-") as directory:
        base = torch.randint(-32, 33, (width, width), generator=generator).float() / 256
        up = torch.randint(-2, 3, (width, rank), generator=generator).float() / 64
        down = torch.randint(-2, 3, (rank, width), generator=generator).float() / 64
        writer = tensorfs.Writer(Path(directory) / "lora.cozytensors")
        component = writer.component("net")
        for key, value in {
            "0.weight": base,
            "0.lora_up": up,
            "0.lora_down": down,
        }.items():
            component.put(key, value)
        checkpoint = writer.commit()

        class LoRA:
            scale = 1.0

            @property
            def identity(self):
                return f"{checkpoint.id}:up+down:{self.scale}:f32-addmm-to-bf16/1"

            def __call__(self, block, parts):
                parts["0.weight"]["value"].addmm_(
                    parts["0.lora_up"]["value"],
                    parts["0.lora_down"]["value"],
                    alpha=self.scale,
                )

        class Bound(FinalWriter):
            def layout(self, key):
                return super().layout(key) if key == "0.weight" else {}

        recipe = LoRA()
        binding_writer = Bound(checkpoint, width, "cast", "BF16")
        module = network(
            checkpoint, width, fill=recipe, finals=binding_writer, dtype=torch.bfloat16
        )
        owner = memory.load(module, "ram")
        binding = owner.retain()
        with memory.request("lora:one"), binding.prepare(), torch.no_grad():
            escaped = module[0].weight[:2]
        before = (base + up @ down).bfloat16()
        torch.testing.assert_close(escaped.cpu(), before[:2], rtol=0, atol=0)
        recipe.scale = 0.5
        assert not binding.valid()
        binding.close()
        inputs = torch.ones(2, width, dtype=torch.bfloat16, device="cuda")
        with memory.request("lora:changed") as changed, torch.no_grad():
            actual = module(inputs).cpu()
        expected = (base + (up @ down) * 0.5).bfloat16()
        torch.testing.assert_close(
            actual,
            torch.nn.functional.linear(inputs.cpu(), expected),
            rtol=1e-2,
            atol=1e-3,
        )
        torch.testing.assert_close(escaped.cpu(), before[:2], rtol=0, atol=0)
        with memory.request("lora:warm") as warm, torch.no_grad():
            module(inputs)
        assert warm.stats()["final_bindings"]["fills"] == 0
        assert warm.stats()["moved"]["store"]["bytes"] == 0
        del escaped
        owner.close()
        memory.trim()
        checkpoint.close()
        return {
            "changed": changed.stats(),
            "warm": warm.stats(),
            "old_fill_alias_preserved": True,
        }


def exercise(seed, mode):
    torch.set_num_threads(1)
    accel.initialize(torch, "cuda")
    memory = residency.Memory.open(
        "cuda:0", budget_bytes=64 * MIB if mode == "stream" else None
    )
    memory.apply_budget()
    output = {
        "seed": seed,
        "mode": mode,
        "torch": torch.__version__,
        "tensorfs": tensorfs.__version__,
        "runtime_version": importlib.metadata.version("cozy-runtime"),
        "runtime_commit": _build_provenance.COMMIT,
        "residency_sha256": hashlib.sha256(
            Path(residency.__file__).read_bytes()
        ).hexdigest(),
        "managed_budget_scope": "Torch allocator and native weight backing; native decoder/library scratch is outside this limit",
    }
    output["result"] = {
        "fits": fits,
        "routes": production_routes,
        "stream": streaming,
        "failure": failed_fill,
        "lora": lora,
    }[mode](memory, seed)
    output["passed"] = True
    return output


if __name__ == "__main__":
    print(json.dumps({"result": exercise(int(sys.argv[1]), sys.argv[2])}), flush=True)
