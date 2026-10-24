"""Actual file -> Torch-owned weights -> inference -> eviction -> refill lifecycle.

CPU checks storage/binding behavior. GPU additionally streams a network larger than its explicitly
local Torch allowance, and captures/replays a fitting network. This is a memory-mechanism cell,
not diffusion/video quality or total-machine-cap qualification. It injects no device faults.
"""

import hashlib
import json
import math
import sys
import tempfile
from pathlib import Path

import tensorfs
import torch
from cozy_runtime.internal import residency
from torch import nn

MIB = 1 << 20


def model(checkpoint, width: int, layers: int, fill=None):
    with torch.device("meta"):
        network = nn.Sequential(*(nn.Linear(width, width, bias=False) for _ in range(layers)))
    vars(network)["_cozy_weights"] = residency.Declared(
        residency.Weights(checkpoint, "net"), blocks=[str(i) for i in range(layers)], fill=fill
    )
    return network


class Scale:
    scale = 1.0

    @property
    def identity(self):
        return f"stock-memory-scale:{self.scale}"

    def __call__(self, block, parts):
        for roles in parts.values():
            roles["value"].mul_(self.scale)


def exercise(device: str, seed: int) -> dict:
    torch.set_num_threads(2)
    cuda = device.startswith("cuda")
    width, layers = (1024, 64) if cuda else (128, 4)
    budget = 160 * MIB if cuda else None
    memory = residency.Memory.open(device, budget_bytes=budget)
    memory.apply_budget()
    generator = torch.Generator().manual_seed(seed)
    source_hash = hashlib.sha256(Path(residency.__file__).read_bytes()).hexdigest()
    result = {
        "device": device, "seed": seed, "torch": torch.__version__, "tensorfs": tensorfs.__version__,
        "residency_sha256": source_hash, "local_allocator_budget": budget,
        "weights_bytes": width * width * 4 * layers,
    }
    with tempfile.TemporaryDirectory(prefix="cozy-stock-memory-") as directory:
        weights = [torch.randn(width, width, generator=generator) / math.sqrt(width) for _ in range(layers)]
        writer = tensorfs.Writer(Path(directory) / "weights.cozytensors")
        component = writer.component("net")
        for index, weight in enumerate(weights):
            component.put(f"{index}.weight", weight)
        checkpoint = writer.commit()
        x_cpu = torch.randn(4, width, generator=generator)
        expected = x_cpu
        for weight in weights:
            expected = torch.nn.functional.linear(expected, weight)
        fill = Scale()
        network = model(checkpoint, width, layers, fill)
        loaded = memory.load(network, "ram")
        x = x_cpu.to(device)

        def forward(module, inputs, name: str):
            with memory.request(name, plan=[module]) as request, torch.no_grad():
                y = module(inputs).cpu()
            return y, request.stats()

        first, cold = forward(network, x, "cold")
        second, warm = forward(network, x, "warm")
        torch.testing.assert_close(first, expected, rtol=2e-4, atol=2e-5)
        torch.testing.assert_close(second, first, rtol=0, atol=0)
        result["cold"] = cold
        result["warm"] = warm
        if not cuda:
            assert warm["moved"]["store"]["bytes"] == 0, "a fitting model must stay warm"

        # An actual tensor view escaping its forward keeps normal Torch storage alive.
        with memory.request("alias"), loaded.passing():
            escaped = network[0].weight[:2]
        memory.load(network, "ram")
        memory.trim()
        torch.testing.assert_close(escaped.cpu(), weights[0][:2], rtol=0, atol=0)
        del escaped
        refilled, refills = forward(network, x, "refilled")
        torch.testing.assert_close(refilled, first, rtol=0, atol=0)
        assert refills["moved"]["store"]["bytes"] > 0
        result["refill"] = refills

        if not cuda:
            binding = loaded.retain()
            with binding.prepare():
                old_addresses = [parameter.data_ptr() for parameter in network.parameters()]
            fill.scale = 0.5
            assert not binding.valid(), "a changed fill cannot reuse its cached graph binding"
            binding.close()
            changed, _ = forward(network, x, "new-fill")
            torch.testing.assert_close(changed, expected * (0.5 ** layers), rtol=2e-5, atol=2e-6)
            result["identity_invalidated"] = bool(old_addresses)
        loaded.close()
        del network, loaded, x
        memory.trim()
        checkpoint.close()

        if cuda:
            # The oversize component above streams eagerly; capture a separate fitting root.
            tiny_width, tiny_layers = 64, 3
            writer = tensorfs.Writer(Path(directory) / "resident.cozytensors")
            component = writer.component("net")
            for index in range(tiny_layers):
                component.put(f"{index}.weight", torch.randn(tiny_width, tiny_width, generator=generator) / 8)
            tiny_ck = writer.commit()
            tiny = model(tiny_ck, tiny_width, tiny_layers)
            loaded = memory.load(tiny, "ram")
            static_input = torch.randn(4, tiny_width, generator=generator).to(device)
            binding = loaded.retain()
            stream = torch.cuda.Stream()
            stream.wait_stream(torch.cuda.current_stream())
            with memory.request("graph"), binding.prepare(stream), torch.cuda.stream(stream), torch.no_grad():
                for _ in range(3):
                    expected_gpu = tiny(static_input)
                stream.synchronize()
                graph = torch.cuda.CUDAGraph()
                with torch.cuda.graph(graph, stream=stream):
                    static_output = tiny(static_input)
            with binding.replay_use(stream), torch.cuda.stream(stream):
                graph.replay()
                actual = static_output.clone()
            stream.synchronize()
            torch.testing.assert_close(actual, expected_gpu, rtol=0, atol=0)
            graph.reset()
            del graph, static_output, actual, expected_gpu
            binding.close()
            memory.load(tiny, "ram")
            loaded.close()
            del loaded, tiny, static_input
            memory.trim()
            tiny_ck.close()
            result["graph_replayed"] = True
            result["allocator"] = torch.cuda.get_allocator_backend()
            assert result["allocator"] == "native"
    assert hashlib.sha256(Path(residency.__file__).read_bytes()).hexdigest() == source_hash
    result["passed"] = True
    return result


if __name__ == "__main__":
    print(json.dumps({"result": exercise(sys.argv[1], int(sys.argv[2]))}), flush=True)
