"""Real TensorFS raw/encoded weights consumed by the resident GPU repeat.

This caller uses ordinary Runtime residency and explicit GPU timestep/noise inputs.
The eager reference is only a numerical oracle; submitted work never invokes its Python step.
"""

from __future__ import annotations

import hashlib
import json
import math
import sys
import tempfile
from pathlib import Path

import cozy_runtime.compute as compute
import numpy as np
import tensorfs
import torch
from cozy_runtime.compute import PreparedRepeat
from cozy_runtime.internal import accel, residency
from tensorfs.raw import decode_into
from tensorfs.raw import parts as stored_parts
from torch import nn


def exercise(seed: int) -> dict:
    torch.set_num_threads(1)
    accel.initialize(torch, "cuda")
    memory = residency.Memory.open("cuda:0")
    generator = torch.Generator().manual_seed(seed)
    width, batch, count = 128, 4, 30
    initial_cpu = torch.randn(batch, width, generator=generator) * 0.1
    noise_cpu = torch.randn(count, batch, width, generator=generator) * 0.005
    schedule_cpu = torch.linspace(0.01, 0.03, count)
    answer = {
        "seed": seed,
        "steps": count,
        "torch": torch.__version__,
        "tensorfs": tensorfs.__version__,
        "residency_sha256": hashlib.sha256(Path(residency.__file__).read_bytes()).hexdigest(),
        "compute_sha256": hashlib.sha256(Path(compute.__file__).read_bytes()).hexdigest(),
        "scope": "resident fixed-shape MLP with real raw and encoded TensorFS weights",
        "cases": {},
    }
    with tempfile.TemporaryDirectory(prefix="cozy-plan-consumer-") as directory:
        for scheme in ("plain", "fp8.tensorwise"):
            writer = tensorfs.Writer(Path(directory) / f"{scheme}.cozytensors")
            component = writer.component("net")
            for layer in (0, 2):
                weight = torch.randn(width, width, generator=generator) / math.sqrt(width)
                key = f"{layer}.weight"
                if scheme == "plain":
                    component.put(key, weight)
                else:
                    scale = np.array(0.125, dtype=np.float32)
                    payload = (weight / float(scale)).to(torch.float8_e4m3fn)
                    component.put_encoded(
                        key,
                        dtype="F32",
                        shape=(width, width),
                        scheme=scheme,
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
            checkpoint = writer.commit()
            metadata = {
                key: stored_parts(checkpoint, "net", key) for key in ("0.weight", "2.weight")
            }
            decode_calls = 0

            def bind(key, physical, scheme=scheme, metadata=metadata):
                nonlocal decode_calls
                if scheme == "plain":
                    return {key: physical["value"]}
                decode_calls += 1
                out = torch.empty((width, width), dtype=torch.float32, device="cuda")
                decode_into(
                    scheme,
                    {
                        role: (meta.dtype, meta.shape, physical[role])
                        for role, meta in metadata[key].items()
                    },
                    out,
                    dtype="F32",
                    shape=(width, width),
                )
                return {key: out}

            with torch.device("meta"):
                network = nn.Sequential(
                    nn.Linear(width, width, bias=False),
                    nn.Tanh(),
                    nn.Linear(width, width, bias=False),
                )
            vars(network)["_cozy_weights"] = residency.Declared(
                residency.Weights(checkpoint, "net"), blocks=["0", "2"], bind=bind
            )
            loaded = memory.load(network, "ram")
            binding = loaded.retain()
            latent = initial_cpu.to("cuda")
            noise = noise_cpu.to("cuda")
            schedule = schedule_cpu.to("cuda")
            calls = 0

            def step(index, network=network, latent=latent, schedule=schedule, noise=noise):
                nonlocal calls
                calls += 1
                delta = network(latent)
                sigma = schedule.index_select(0, index).reshape(1, 1)
                increment = noise.index_select(0, index).reshape(batch, width)
                latent.add_(delta * sigma + increment)

            plan = None
            try:
                with memory.request(f"{scheme}:prepare"), binding.prepare(), torch.no_grad():
                    index = torch.zeros(1, dtype=torch.int64, device="cuda")
                    for iteration in range(count):
                        index.fill_(iteration)
                        step(index)
                    expected = latent.clone()
                    latent.copy_(initial_cpu.to("cuda"))
                    plan = PreparedRepeat.capture(
                        step,
                        state=(latent,),
                        steps=count,
                        owners=(binding, checkpoint, network, noise, schedule),
                    )
                    prepared_calls = calls
                    prepared_decodes = decode_calls
                binding.close()  # the prepared definition owns a separate retained lease
                first = plan.submit()
                (actual,) = first.wait()
                torch.testing.assert_close(actual, expected, rtol=1e-5, atol=1e-6)
                assert first.poll().completed_steps == count
                assert calls == prepared_calls and decode_calls == prepared_decodes
                saved_output = actual.clone()
                latent.copy_(initial_cpu.to("cuda"))
                second = plan.submit()
                (again,) = second.wait()
                torch.testing.assert_close(again, expected, rtol=1e-5, atol=1e-6)
                torch.testing.assert_close(actual, saved_output, rtol=0, atol=0)
                assert calls == prepared_calls and decode_calls == prepared_decodes
                answer["cases"][scheme] = {
                    "completed_steps": [
                        first.poll().completed_steps,
                        second.poll().completed_steps,
                    ],
                    "python_step_calls_after_prepare": calls - prepared_calls,
                    "decode_calls_after_prepare": decode_calls - prepared_decodes,
                    "prepared_decodes": prepared_decodes,
                    "max_abs_error": float((again - expected).abs().max().item()),
                    "metadata": {
                        key: {
                            role: {"dtype": meta.dtype, "shape": meta.shape, "bytes": meta.length}
                            for role, meta in values.items()
                        }
                        for key, values in metadata.items()
                    },
                    "decoder_scratch_bound": None if scheme != "plain" else 0,
                    "managed_budget_scope": (
                        "Torch allocations and native weight backing; "
                        "native decoder scratch excluded"
                    ),
                    "native_execution": plan.facts(),
                }
            finally:
                if plan is not None:
                    plan.close()
                binding.close()
                loaded.close()
                memory.trim()
                checkpoint.close()
    answer["passed"] = True
    return answer


if __name__ == "__main__":
    print(json.dumps({"result": exercise(int(sys.argv[1]))}), flush=True)
