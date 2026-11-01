"""Real sparse-producer reductions; complete consumer qualification is a separate cell."""
from __future__ import annotations

import gc
import hashlib
import json
import os
from importlib import import_module
from importlib.metadata import version
from pathlib import Path

import msgspec
import torch
from cozy_runtime.author import App, Context
from minimax_h3 import glue, producer_kernels

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    runtime_version: str
    runtime_modules: dict[str, str]
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    gpu_uuid: str
    package_modules: dict[str, str]
    kernel_identities: dict[str, str]
    seed: int = 13829


class Cleanup(Probe, kw_only=True):
    expected_pid: int
    keys: list[str] = msgspec.field(default_factory=list)


class Report(msgspec.Struct):
    passed: bool
    detail: str


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def actual(payload, ctx):
    import cozy_runtime
    import tensorfs
    from cozy_runtime.internal.residency import _REQUEST, Request
    from tensorfs import _ext
    from tensorfs.varena import torch as vt

    assert version('cozy-runtime') == payload.runtime_version
    root = Path(cozy_runtime.__file__).resolve().parent
    assert set(payload.runtime_modules) == {
        'author/kernels.py', 'internal/triton_kernels.py', 'internal/attention.py',
        'internal/kernel_sources.py', 'internal/kernel_ready.py',
        'author/_split.py', 'internal/execution/native_sparse_producer.py',
        'internal/execution/native_linear_split.py', 'internal/execution/phase.py',
        'internal/residency.py', 'internal/producers.py',
    }
    for name, expected in payload.runtime_modules.items():
        assert digest(root / name) == expected, name
    root = Path(glue.__file__).resolve().parent
    assert set(payload.package_modules) == {
        '__init__.py', 'glue.py', 'adaln_pruned.py', 'overlay.py', 'producer_kernels.py', 'producer.py',
    }
    for name, expected in payload.package_modules.items():
        assert digest(root / name) == expected, name
    assert payload.kernel_identities == {
        s.name: s.identity for s in (glue.GLUE, glue.GLUE_FP8, producer_kernels.STATS)}
    assert version('tensorfs') == payload.tensorfs_version
    assert digest(_ext.__file__) == payload.extension_sha256
    assert digest(Path(tensorfs.__file__).resolve().parent / '_varena_torch.so') == payload.shim_sha256
    assert str(torch.__version__) == '2.14.0+cu130'
    assert torch.version.git_version == '08187d9e0fba026dc8217405802ab5381dc88d90'
    assert ctx.device.type == 'cuda' and torch.cuda.device_count() == 1
    uuid = str(torch.cuda.get_device_properties(0).uuid)
    assert uuid.removeprefix('GPU-').lower() == payload.gpu_uuid.removeprefix('GPU-').lower()
    request = _REQUEST.get()
    assert isinstance(request, Request) and request.id == ctx.request_id and not request.idle
    assert request in request.memory.requests and not request.memory.models
    return request, vt, {**msgspec.to_builtins(payload), 'pid': os.getpid(),
                         'request_id': ctx.request_id, 'gpu_uuid': uuid}


def settle(vt, device):
    gc.collect()
    torch.cuda.synchronize(device)
    vt.trim(device)
    torch.cuda.synchronize(device)
    vt.trim(device)


def capsule(tensor):
    return tensor.__dlpack__(stream=-1)


def measured(vt, device, operation):
    before = vt.stats(device)
    operation()
    torch.cuda.current_stream(device).synchronize()
    after = vt.stats(device)
    delta = {key: after[key] - before[key]
             for key in ('allocations', 'frees', 'driver_memory_calls')}
    assert delta == dict.fromkeys(delta, 0), delta
    return delta


def one_case(native, launch, vt, device, generator, tokens, heads):
    """Use real Kitchen-produced post-RoPE block sums, and the actual Torch recipe."""
    from comfy_kitchen.backends import cuda as kitchen
    from cozy_runtime.internal import prebuilt, verify

    on, width = torch.device('cuda', device), heads * 128
    qkv = torch.randn((tokens, 3 * width), generator=generator, dtype=torch.bfloat16).to(on)
    # Include the value-scale floor in every geometry, alongside signed finite values.
    qkv[:, 2 * width] = 0
    values = qkv[:, 2 * width:].contiguous()
    found = None
    for first in range(0, tokens, 4096):
        part = values[first:first + 4096]
        peak = torch.linalg.vector_norm(part, ord=float('inf'), dim=0)
        found = peak if found is None else torch.maximum(found, peak)
    expected_max = found.float().cpu()
    expected_scale = (found.float() / 127.0 * 1.1).clamp_min(1e-8).cpu()
    del found, part, peak
    maximum = torch.empty((heads, 128), dtype=torch.float32, device=on)

    def value_maximum():
        maximum.zero_()
        for first in range(0, tokens, 4096):
            part = values[first:first + 4096]
            launch(producer_kernels.VALUE_ABSMAX.kernel,
                   ((part.shape[0] + 63) // 64, (width + 31) // 32, 1),
                   part, maximum, part.shape[0], width, part.stride(0), 64, 32)

    maximum_delta = measured(vt, device, value_maximum)
    actual_max = maximum.cpu().reshape(-1)
    assert torch.equal(actual_max, expected_max)

    def value_scale():
        maximum.div_(127.0).mul_(1.1).clamp_min_(1e-8)

    scale_delta = measured(vt, device, value_scale)
    assert torch.equal(maximum.cpu().reshape(-1), expected_scale)
    assert expected_scale[0] == torch.tensor(1e-8, dtype=torch.float32)

    plan = native.sol_attn_plan(1, tokens, heads, token_aug=0)
    workspace = torch.empty(plan['total'], dtype=torch.uint8, device=on)
    rotary = torch.zeros((tokens, 96, 2), dtype=torch.float32, device=on)
    rotary[:, :, 0] = 1
    norm = torch.ones(128, dtype=torch.bfloat16, device=on)
    mean = torch.zeros((heads, 128), dtype=torch.float32, device=on)
    scale = torch.ones_like(mean)
    denominator = torch.full((), float(tokens), dtype=torch.float32, device=on)
    stream = int(torch.cuda.current_stream(device).cuda_stream)
    native.sol_producer_begin(capsule(workspace), 1, tokens, heads, stream, token_aug=0)
    for first in range(0, tokens, 4096):
        part = qkv[first:first + 4096]
        native.sol_producer_chunk(
            capsule(workspace), capsule(part), capsule(rotary), capsule(norm), capsule(norm),
            capsule(mean), capsule(scale), 1e-5, 96, first, part.shape[0], 1, tokens, heads,
            stream, block_len=None, token_aug=0)
    sums = kitchen._ws_ksums(workspace, plan, heads)
    lengths = kitchen._block_lengths(tokens, plan['NTB'], on, None)
    expected_mean = (sums.sum(1) / lengths.sum()).cpu()
    # Validate the actual per-channel maximum the pinned producer put in its workspace.
    vmax = workspace[plan['statsV']:plan['statsV'] + heads * 128 * 4].view(torch.float32)
    assert torch.equal(vmax.cpu(), expected_max)

    def key_mean():
        for head in range(heads):
            # The existing kernel's head index is zero here; the pointer supplies that
            # head's padded workspace stride. It reads only NTB actual block sums.
            launch(producer_kernels.KEY_SUM.kernel, (1, 1, 1), sums[head], mean[head],
                   plan['NTB'], sums.stride(1), 128, 64)
        mean.div_(denominator)

    mean_delta = measured(vt, device, key_mean)
    actual_mean = mean.cpu()
    error = verify.error(torch, actual_mean, expected_mean)
    assert torch.isfinite(actual_mean).all() and error <= prebuilt.BUDGET
    return {'tokens': tokens, 'heads': heads, 'chunks': [min(4096, tokens - first)
            for first in range(0, tokens, 4096)], 'native_workspace': plan,
            'value_absmax_exact': True, 'value_scale_exact': True,
            'native_workspace_vmax_exact': True,
            'mean': {'exact': torch.equal(actual_mean, expected_mean),
                     'relative_l2': error, 'budget': prebuilt.BUDGET},
            'allocation_deltas': {'maximum': maximum_delta, 'scale': scale_delta, 'mean': mean_delta}}


@app.entrypoint(accelerator=True)
def reductions(payload: Probe, ctx: Context) -> Report:
    from cozy_runtime.internal import attention, triton_kernels
    from cozy_runtime.internal.encoding.selection import measure_device

    request, vt, provenance = actual(payload, ctx)
    device = ctx.device.index or 0
    torch.set_num_threads(1)
    ready, = attention.pinned('kitchen-int8', measure_device(torch, device, 'cuda'))
    from comfy_kitchen.backends import cuda as kitchen
    source = import_module('comfy_kitchen.sage_attention')
    assert version('comfy-kitchen') == '0.2.40'
    assert digest(source.__file__) == '23447a19aa113722f9b0e9a3b1fcc30cec59c438fe652535b870922f50d0543a'
    assert all(callable(getattr(kitchen._C, name, None)) for name in
               ('sol_attn_plan', 'sol_producer_begin', 'sol_producer_chunk'))
    launch = triton_kernels.load(producer_kernels.STATS, device=torch.device('cuda', device))
    provenance.update(launcher_key=launch.key, kitchen_artifact=ready.artifact,
                      kitchen_extension_sha256=digest(kitchen._C.__file__))
    generator = torch.Generator().manual_seed(payload.seed)
    settle(vt, device)
    initial = vt.stats(device)
    with torch.inference_mode():
        cases = [one_case(kitchen._C, launch, vt, device, generator, tokens, heads)
                 for heads in (56, 28, 1) for tokens in (4096, 4099, 8193, 8195)]
    settle(vt, device)
    final = vt.stats(device)
    for field in ('allocated_bytes', 'allocated_capacity', 'mapped_bytes', 'pending_bytes',
                  'pools', 'pools_bytes', 'event_handles'):
        assert final[field] == initial[field], (field, initial, final)
    assert not request.working_phase_keys
    return Report(True, json.dumps({'passed': True, 'scope': 'bounded reduction kernels only; not a complete producer',
                                   'provenance': provenance, 'cases': cases,
                                   'initial': initial, 'final': final}, sort_keys=True))


@app.entrypoint(accelerator=True)
def cleanup(payload: Cleanup, ctx: Context) -> Report:
    request, vt, provenance = actual(payload, ctx)
    assert os.getpid() == payload.expected_pid and not request.working_phase_keys
    settle(vt, ctx.device.index or 0)
    retired = {key: vt.phase_forget(ctx.device.index or 0, key) for key in payload.keys}
    assert not any(retired.values()), retired
    return Report(True, json.dumps({'passed': True, 'provenance': provenance,
                                   'prior_request_keys_already_retired': retired,
                                   'stats': vt.stats(ctx.device.index or 0)}, sort_keys=True))


from .consumer import consumer as consumer
