"""Observe actual H3 prefix Linear layouts and rounding; no fake native provider."""
from __future__ import annotations

import hashlib
import inspect
import json
from pathlib import Path

import torch
from diffusers.models.autoencoders.autoencoder_kl_minimax_h3 import (
    MiniMaxH3VideoViTDecoder3d, MiniMaxH3VideoRotaryPosEmbed,
)
from torch.profiler import ProfilerActivity, profile

assert torch.version.git_version == '08187d9e0fba026dc8217405802ab5381dc88d90'
torch.set_num_threads(1)
torch.manual_seed(1733)
with torch.device('meta'):
    decoder = MiniMaxH3VideoViTDecoder3d()
decoder.eval().requires_grad_(False)
module = decoder.proj_in.to_empty(device='cpu')
rows = []
with torch.no_grad():
    for dtype in (torch.float16, torch.float32):
        module.weight.data = module.weight.data.to(dtype)
        module.weight.normal_(0, 0.02)
        module.bias.uniform_(-0.05, 0.05)
        for batch in (1, 3, 7):
            x = torch.randn(batch, 24, 7, 16, 16, dtype=torch.float16) * 0.125
            view = x.permute(0, 2, 3, 4, 1).reshape(batch, 1792, 24)
            assert not view.is_contiguous() and torch._C._is_alias_of(view, x)
            with torch.autocast('cpu', dtype=torch.float16):
                with profile(activities=[ProfilerActivity.CPU]) as observed:
                    actual = module(view)
            packed = view.contiguous().view(-1, 24)
            with profile(activities=[ProfilerActivity.CPU]) as packed_observed:
                fused = torch.nn.functional.linear(packed, module.weight.half(), module.bias.half()).view(batch, 1792, 2048)
            # A separate real half matrix multiply plus half bias add preserves
            # the stock fallback's two explicit operations on this actual CPU path.
            separate = torch.mm(packed, module.weight.half().t())
            separate.add_(module.bias.half())
            separate = separate.view(batch, 1792, 2048)
            actual_ops = [event.name for event in observed.events()]
            packed_ops = [event.name for event in packed_observed.events()]
            assert 'aten::matmul' in actual_ops and 'aten::add_' in actual_ops
            assert 'aten::addmm' not in actual_ops and 'aten::addmm' in packed_ops
            stock_layout = torch.matmul(view, module.weight.half().t())
            stock_layout.add_(module.bias.half())
            torch.testing.assert_close(stock_layout, actual, rtol=0, atol=0)
            torch.testing.assert_close(fused, actual, rtol=0.002, atol=0.0001)
            relative_l2 = float(torch.linalg.vector_norm(fused.float()-actual.float()) / torch.linalg.vector_norm(actual.float()))
            assert relative_l2 <= 0.001
            rows.append({'batch': batch, 'weight_dtype': str(dtype), 'input_shape': list(x.shape),
                         'token_view_strides': list(view.stride()), 'view_aliases_input': True,
                         'stock_ops': actual_ops, 'packed_fused_ops': packed_ops,
                         'stock_layout_control_bit_exact': True, 'stock_vs_packed_separate_bit_exact': torch.equal(separate,actual),
                         'packed_separate_max_abs': float((separate.float()-actual.float()).abs().max()),
                         'fused_relative_l2': relative_l2, 'fused_bounds': {'rtol':0.002,'atol':0.0001,'relative_l2':0.001}, 'stock_vs_fused_bit_exact': torch.equal(actual, fused),
                         'fused_differing_values': int((actual != fused).sum()),
                         'fused_max_abs': float((actual.float()-fused.float()).abs().max())})
            del x, view, actual, packed, fused, separate, stock_layout
assert not torch.cuda.is_initialized() and 'libcuda.so' not in Path('/proc/self/maps').read_text()
print(json.dumps({'passed': True, 'torch': torch.__version__, 'torch_commit': torch.version.git_version,
                  'scope': 'actual CPU operator path and round-point audit; no native provider or CUDA qualification',
                  'decoder_source_sha256': hashlib.sha256(inspect.getsource(MiniMaxH3VideoViTDecoder3d).encode()).hexdigest(),
                  'rope_source_sha256': hashlib.sha256(inspect.getsource(MiniMaxH3VideoRotaryPosEmbed).encode()).hexdigest(),
                  'cases': rows, 'cuda_initialized': False, 'gpu_qualified': False}, indent=2))
