"""Real distinct-Storage shared-byte and DLPack aliases; no CUDA/provider substitute."""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import sys

import torch

parser = argparse.ArgumentParser()
parser.add_argument('--runtime-source', type=Path, required=True)
args = parser.parse_args()
torch.set_num_threads(1)
rows = []
for name in ('native_vae_post_quant', 'native_vae_output', 'native_vae_prefix'):
    path = args.runtime_source / 'src/cozy_runtime/internal/execution' / (name + '.py')
    spec = importlib.util.spec_from_file_location('span_' + name, path)
    consumer = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = consumer
    spec.loader.exec_module(consumer)
    shared = bytearray(128)
    left = torch.frombuffer(shared, dtype=torch.float32, count=16)
    duplicate = torch.frombuffer(shared, dtype=torch.float32, count=16)
    partial = torch.frombuffer(shared, dtype=torch.float16, count=16, offset=16)
    adjacent = torch.frombuffer(shared, dtype=torch.float32, count=16, offset=64)
    assert not torch._C._is_alias_of(left, duplicate)
    assert not torch._C._is_alias_of(left, partial)
    assert not torch._C._is_alias_of(left, adjacent)
    assert consumer._overlap(left, duplicate) and consumer._overlap(left, partial)
    assert not consumer._overlap(left, adjacent)
    assert consumer._overlap(left[:8], left[8:])  # Preserve the prior Storage-alias refusal.
    assert not consumer._overlap(left, torch.zeros_like(left))
    left.fill_(7)
    assert bool((duplicate == 7).all())
    dl = torch.from_dlpack(left)
    assert not torch._C._is_alias_of(left, dl)
    assert dl.data_ptr() == left.data_ptr() and consumer._overlap(left, dl)
    noncontiguous_refusal = None
    if name != 'native_vae_prefix':
        try:
            consumer._overlap(left, left[::2])
            raise AssertionError('noncontiguous span accepted')
        except ValueError as error:
            noncontiguous_refusal = str(error)
    rows.append({'consumer':name,'source_sha256':hashlib.sha256(path.read_bytes()).hexdigest(),
                 'storage_identity_detects_duplicate':False,'address_span_detects_duplicate':True,
                 'partial_overlap_detected':True,'adjacent_independent_spans_accepted':True,
                 'writes_share_real_bytes':True,'actual_cpu_dlpack_alias_detected':True,
                 'noncontiguous_helper_refusal':noncontiguous_refusal,
                 'scope':'CPU pointer predicate only; CUDA binding/ownership is unrun'})
assert not torch.cuda.is_initialized() and 'libcuda.so' not in Path('/proc/self/maps').read_text()
print(json.dumps({'passed':True,'torch':torch.__version__,'cases':rows,'cuda_initialized':False,
                  'gpu_qualified':False},indent=2))
