"""Actual H3 tail dimensions/modules, CPU projection and exact pixel copy; no mocks."""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import sys

import torch
from diffusers.models.autoencoders.autoencoder_kl_minimax_h3 import MiniMaxH3VideoViTDecoder3d
from torch.utils._python_dispatch import TorchDispatchMode

parser = argparse.ArgumentParser()
parser.add_argument('--consumer', required=True, type=Path)
args = parser.parse_args()
spec = importlib.util.spec_from_file_location('tail_consumer', args.consumer)
consumer = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = consumer
spec.loader.exec_module(consumer)
assert torch.version.git_version == consumer.TORCH_COMMIT
torch.set_num_threads(1)
torch.manual_seed(1732)


class Observe(TorchDispatchMode):
    def __init__(self):
        self.calls = []

    def __torch_dispatch__(self, func, types, args=(), kwargs=None):
        self.calls.append(str(func))
        return func(*args, **(kwargs or {}))


with torch.device('meta'):
    decoder = MiniMaxH3VideoViTDecoder3d()
norm, projection = decoder.norm_out, decoder.proj_out
norm.to_empty(device='cpu').eval().requires_grad_(False)
projection.to_empty(device='cpu').eval().requires_grad_(False)
assert decoder.patch_size == 16 and decoder.patch_size_t == 4 and decoder.out_channels == 3
assert decoder.num_register_tokens == 4 and projection.weight.shape == (3072, 2048)
rows = []
with torch.no_grad():
    norm.weight.fill_(1)
    norm.bias.uniform_(-0.01, 0.01)
    for weight_dtype in (torch.float16, torch.float32):
        projection.weight.data = projection.weight.data.to(weight_dtype)
        projection.weight.normal_(0, 0.02)
        projection.bias.uniform_(-0.01, 0.01)
        for batch in (1, 3):
            value = torch.randn(batch, 1797, 2048, dtype=torch.float32) * 0.125
            description = consumer.describe(norm, projection, value, geometry=(7, 16, 16))
            assert description is not None
            assert consumer.prepare(description, context=None, stream=None) is None
            buffers = consumer.Buffers.allocate(description, 0)
            norm_values = torch.ops.aten.native_layer_norm.default(value, [2048], norm.weight, norm.bias, norm.eps)
            torch.testing.assert_close(norm_values[0], norm(value), rtol=0, atol=0)
            with torch.autocast('cpu', dtype=torch.float16):
                reference_rows = projection(norm(value).to(projection.weight.dtype))
            observer = Observe()
            with observer:
                buffers.input_rows.copy_(norm_values[0].view(-1, 2048))
                if buffers.weight_cast is not None:
                    buffers.weight_cast.copy_(projection.weight)
            matrix = projection.weight if buffers.weight_cast is None else buffers.weight_cast
            # Actual CPU linear arithmetic is a host comparison, never a fake Lt Plan.
            dense = torch.nn.functional.linear(buffers.input_rows, matrix, projection.bias.half())
            torch.testing.assert_close(dense.view(batch, 1797, 3072), reference_rows, rtol=0, atol=0)
            reference_pixels = reference_rows[:, :1792].view(batch, 7, 16, 16, 3, 4, 16, 16)
            reference = reference_pixels.permute(0, 4, 1, 5, 2, 6, 3, 7).contiguous().reshape(batch, 3, 28, 256, 256)
            with observer:
                consumer.assemble(dense, buffers.result)
            torch.testing.assert_close(buffers.result, reference, rtol=0, atol=0)
            assert not any('empty' in op or 'clone' in op or '_to_copy' in op for op in observer.calls)
            inventory = {name: tensor.numel() * tensor.element_size()
                         for name, tensor in vars(buffers).items() if tensor is not None}
            assert inventory == dict(description.preallocations(0))
            functional = {name: tensor.numel() * tensor.element_size()
                          for (name, _), tensor in zip(description.functional_allocations, norm_values, strict=True)}
            assert functional == dict(description.functional_allocations)
            assert {**inventory, **functional} == dict(description.allocations(0))
            rows.append({'batch': batch, 'weight_dtype': str(weight_dtype), 'input_dtype': str(value.dtype),
                         'result_shape': list(buffers.result.shape), 'result_dtype': str(buffers.result.dtype),
                         'bit_exact': True, 'preallocations': inventory, 'functional_allocations': functional,
                         'copy_ops': observer.calls})
            del buffers, norm_values, reference_rows, reference_pixels, reference, value, dense, matrix

    value = torch.randn(1, 1797, 2048)
    refused = []
    for name, candidate, geometry in (
        ('fp16_residual', value.half(), (7, 16, 16)),
        ('wrong_geometry', value, (6, 16, 16)),
        ('missing_suffix', value[:, :1792], (7, 16, 16)),
        ('noncontiguous', value.transpose(1, 2).contiguous().transpose(1, 2), (7, 16, 16)),
        ('empty_batch', value[:0], (7, 16, 16)),
    ):
        assert consumer.describe(norm, projection, candidate, geometry=geometry) is None
        refused.append(name)
    norm.eps = 1e-6
    assert consumer.describe(norm, projection, value, geometry=(7, 16, 16)) is None
    refused.append('changed_epsilon')

assert not torch.cuda.is_initialized() and 'libcuda.so' not in Path('/proc/self/maps').read_text()
print(json.dumps({'passed': True, 'torch': torch.__version__, 'torch_commit': torch.version.git_version,
                  'consumer_sha256': hashlib.sha256(args.consumer.read_bytes()).hexdigest(),
                  'scope': 'actual pinned CPU LayerNorm and linear arithmetic plus production allocation metadata/assembly',
                  'cuda_initialized': False, 'gpu_qualified': False, 'cases': rows, 'refusals': refused}, indent=2))
