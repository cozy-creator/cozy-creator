"""Actual pinned Torch LayerNorm allocation and H3 pixel-layout probe; no mocks."""
from __future__ import annotations

import json
from pathlib import Path

import torch
from torch.profiler import ProfilerActivity, profile

assert torch.version.git_version == 'd0f2af8b66b20da4baa50644c0cd3f887a2eae2e'
torch.set_num_threads(1)
torch.manual_seed(1732)
rows = []
with torch.no_grad():
    for batch in (1, 3):
        x = torch.randn(batch, 1797, 2048, dtype=torch.float32)
        module = torch.nn.LayerNorm(2048, eps=1e-5).eval().requires_grad_(False)
        expected = module(x)
        with profile(activities=[ProfilerActivity.CPU], profile_memory=True) as observed:
            values = torch.ops.aten.native_layer_norm.default(x, [2048], module.weight, module.bias, module.eps)
        torch.testing.assert_close(values[0], expected, rtol=0, atol=0)
        sizes = [value.numel() * value.element_size() for value in values]
        assert sizes == [batch * 1797 * 2048 * 4, batch * 1797 * 4, batch * 1797 * 4]
        outs = tuple(torch.empty_like(value) for value in values)
        with profile(activities=[ProfilerActivity.CPU], profile_memory=True) as out_observed:
            returned = torch.ops.aten.native_layer_norm.out(x, [2048], module.weight, module.bias, module.eps,
                                                         out0=outs[0], out1=outs[1], out2=outs[2])
        for result, reference, destination in zip(returned, values, outs, strict=True):
            assert result.data_ptr() == destination.data_ptr()
            torch.testing.assert_close(result, reference, rtol=0, atol=0)
        def allocations(profiler):
            return [{'op': event.name, 'bytes': event.cpu_memory_usage}
                    for event in profiler.events() if event.name == 'aten::empty' and event.cpu_memory_usage > 0]
        functional_allocations, out_allocations = allocations(observed), allocations(out_observed)
        assert sorted(event['bytes'] for event in functional_allocations) == sorted(sizes)
        assert sorted(event['bytes'] for event in out_allocations) == sorted(sizes)
        rows.append({'batch': batch, 'shape': list(x.shape), 'dtypes': [str(v.dtype) for v in values],
                     'shapes': [list(v.shape) for v in values], 'functional_allocations': functional_allocations,
                     'out_wrapper_allocations': out_allocations, 'out_is_zero_allocation': False})
        del x, expected, values, outs, returned

assert not torch.cuda.is_initialized() and 'libcuda.so' not in Path('/proc/self/maps').read_text()
print(json.dumps({'passed': True, 'torch': torch.__version__, 'torch_commit': torch.version.git_version,
                  'scope': 'actual CPU allocation corroboration of the exact pinned native schema; CUDA source reviewed separately',
                  'cuda_initialized': False, 'gpu_qualified': False, 'cases': rows}, indent=2))
