"""Actual H3 prefix comparison: declared fused-bias alternative, exact packing/RoPE."""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import math
from pathlib import Path
import sys

import torch
from diffusers.models.autoencoders.autoencoder_kl_minimax_h3 import (
    MiniMaxH3VideoViTDecoder3d, MiniMaxH3VideoRotaryPosEmbed,
)
from torch.utils._python_dispatch import TorchDispatchMode

parser = argparse.ArgumentParser()
parser.add_argument('--consumer', required=True, type=Path)
args = parser.parse_args()
spec = importlib.util.spec_from_file_location('prefix_consumer', args.consumer)
consumer = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = consumer
spec.loader.exec_module(consumer)
assert torch.version.git_version == consumer.TORCH_COMMIT
torch.set_num_threads(1)
torch.manual_seed(1734)


class Observe(TorchDispatchMode):
    def __init__(self):
        self.calls = []

    def __torch_dispatch__(self, func, types, args=(), kwargs=None):
        self.calls.append(str(func))
        return func(*args, **(kwargs or {}))


with torch.device('meta'):
    decoder = MiniMaxH3VideoViTDecoder3d()
decoder.eval().requires_grad_(False)
projection = decoder.proj_in.to_empty(device='cpu')
decoder.register_tokens = torch.nn.Parameter(torch.randn(1, 4, 2048), requires_grad=False)
decoder.rope = MiniMaxH3VideoRotaryPosEmbed(48).eval().requires_grad_(False)
rows = []
with torch.no_grad():
    for weight_dtype in (torch.float16, torch.float32):
        projection.weight.data = projection.weight.data.to(weight_dtype)
        projection.weight.normal_(0, 0.02)
        projection.bias.uniform_(-0.05, 0.05)
        for batch in (1, 3, 7):
            value = torch.randn(batch, 24, 7, 16, 16, dtype=torch.float16) * 0.125
            description = consumer.describe(decoder, value, geometry=(7, 16, 16))
            assert description is not None
            assert consumer.prepare(description, context=None, stream=None) is None
            buffers = consumer.Buffers.allocate(description, 0)
            before = {name: (t.data_ptr(), tuple(t.shape), tuple(t.stride())) for name, t in buffers.values.items()}
            with torch.autocast('cpu', dtype=torch.float16):
                stock_projected = projection(value.permute(0, 2, 3, 4, 1).reshape(batch, 1792, 24))
            observer = Observe()
            with observer:
                consumer.pack_input(value, buffers['input_rows'])
                if description.weight_cast:
                    buffers['weight_cast'].copy_(projection.weight)
            matrix = buffers['weight_cast'] if description.weight_cast else projection.weight
            # Real CPU arithmetic only, never a fake native provider.
            candidate = torch.nn.functional.linear(buffers['input_rows'], matrix, projection.bias.half())
            buffers['dense_result'].copy_(candidate)
            with observer:
                hidden, (cosine, sine) = consumer.pack_tokens_and_rotary(decoder.register_tokens, decoder.rope.inv_freq, buffers)
            registers = decoder.register_tokens.expand(batch, -1, -1).to(torch.float16)
            expected_packing = torch.cat((candidate.view(batch, 1792, 2048), registers,
                                          torch.zeros(batch, 1, 2048, dtype=torch.float16)), dim=1)
            torch.testing.assert_close(hidden, expected_packing, rtol=0, atol=0)
            stock_hidden = torch.cat((stock_projected, registers, torch.zeros_like(stock_projected[:, :1])), dim=1)
            torch.testing.assert_close(hidden, stock_hidden, rtol=0.002, atol=0.0001)
            relative_l2 = float(torch.linalg.vector_norm(candidate.float()-stock_projected.flatten(0, 1).float())
                                / torch.linalg.vector_norm(stock_projected.float()))
            assert relative_l2 <= 0.001
            grids = [2.0 * (torch.arange(0.5, size, dtype=torch.float32) / size) - 1.0 for size in (7, 16, 16)]
            positions = torch.stack(torch.meshgrid(*grids, indexing='ij'), dim=-1).flatten(0, 2)[None]
            positions = torch.cat((positions, positions.new_zeros(1, 5, 3)), dim=1)
            stock_cosine, stock_sine = decoder.rope(positions)
            torch.testing.assert_close(cosine, stock_cosine, rtol=0, atol=0)
            torch.testing.assert_close(sine, stock_sine, rtol=0, atol=0)
            torch.testing.assert_close(buffers['positions'], positions, rtol=0, atol=0)
            scaled = 2.0 * math.pi * positions[..., None]
            angles = scaled * decoder.rope.inv_freq[None, None, None, :]
            torch.testing.assert_close(buffers['scaled_positions'], scaled, rtol=0, atol=0)
            torch.testing.assert_close(buffers['angles'], angles, rtol=0, atol=0)
            torch.testing.assert_close(buffers['tiled_angles'], angles.flatten(2, 3).tile(2).unsqueeze(2), rtol=0, atol=0)
            after = {name: (t.data_ptr(), tuple(t.shape), tuple(t.stride())) for name, t in buffers.values.items()}
            assert before == after
            assert not any('empty' in op or 'clone' in op or '_to_copy' in op for op in observer.calls)
            inventory = {name: t.numel() * t.element_size() for name, t in buffers.values.items()}
            assert inventory == dict(description.allocations(0))
            rows.append({'batch': batch, 'matrix_dtype': str(weight_dtype), 'implementation': consumer.IMPLEMENTATION,
                         'projection_alternative': True, 'projection_relative_l2': relative_l2,
                         'projection_max_abs': float((candidate.float()-stock_projected.flatten(0, 1).float()).abs().max()),
                         'projection_bounds': {'rtol':0.002,'atol':0.0001,'relative_l2':0.001},
                         'packing_bit_exact': True, 'rope_and_intermediates_bit_exact': True,
                         'hidden_shape': list(hidden.shape), 'rotary_shape': list(cosine.shape),
                         'actual_inventory': inventory, 'operations': observer.calls})
            del buffers, value, candidate, matrix, hidden, cosine, sine, stock_hidden, stock_projected, expected_packing

    value = torch.randn(1, 24, 7, 16, 16, dtype=torch.float16)
    refused = []
    for name, x, geometry in (('fp32_input', value.float(), (7,16,16)),
                              ('channels_last', value.to(memory_format=torch.channels_last_3d), (7,16,16)),
                              ('wrong_geometry', value, (6,16,16)),
                              ('empty_batch', value[:0], (7,16,16))):
        assert consumer.describe(decoder, x, geometry=geometry) is None
        refused.append(name)
    decoder.rope.inv_freq = decoder.rope.inv_freq.half()
    assert consumer.describe(decoder, value, geometry=(7,16,16)) is None
    refused.append('fp16_frequency')

shared = bytearray(64)
alias_a = torch.frombuffer(shared, dtype=torch.float32)
alias_b = torch.frombuffer(shared, dtype=torch.float32)
assert not torch._C._is_alias_of(alias_a, alias_b)
assert alias_a.data_ptr() == alias_b.data_ptr() and consumer._overlap(alias_a, alias_b)
alias_a.fill_(7)
assert bool((alias_b == 7).all())
assert not torch.cuda.is_initialized() and 'libcuda.so' not in Path('/proc/self/maps').read_text()
print(json.dumps({'passed': True, 'torch': torch.__version__, 'torch_commit': torch.version.git_version,
                  'consumer_sha256': hashlib.sha256(args.consumer.read_bytes()).hexdigest(),
                  'scope': 'actual CPU fused-bias projection alternative plus exact production packing/RoPE and inventory',
                  'cuda_initialized': False, 'gpu_qualified': False, 'cases': rows, 'refusals': refused,
                  'independent_storage_physical_alias_detected': True}, indent=2))
