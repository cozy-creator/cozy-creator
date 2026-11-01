"""Actual package _Tail.run versus one complete native tail, through real eager owners."""
from __future__ import annotations

import gc
import hashlib
import json
import os
from dataclasses import replace
from importlib.metadata import version
from pathlib import Path

import msgspec
import torch
from cozy_runtime.author import App, Context, Cut, denoise_loop, split
from minimax_h3 import glue

app = App()
ROWS, WIDTH, INNER = 8195, 5376, 14336


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    runtime_version: str
    runtime_modules: dict[str, str]
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    gpu_uuid: str
    package_modules: dict[str, str]
    glue_identities: dict[str, str]
    seed: int = 13827


class Cleanup(Probe, kw_only=True):
    expected_pid: int
    keys: list[str]


class Report(msgspec.Struct):
    passed: bool
    detail: str


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def actual_request(payload, ctx):
    import cozy_runtime
    import tensorfs
    from cozy_runtime.internal.residency import _REQUEST, Request
    from tensorfs import _ext
    from tensorfs.varena import torch as vt

    assert version('cozy-runtime') == payload.runtime_version
    base = Path(cozy_runtime.__file__).resolve().parent
    assert set(payload.runtime_modules) == {
        'author/kernels.py', 'author/_split.py', 'internal/encoding/leaves.py',
        'internal/prebuilt.py', 'internal/execution/__init__.py',
        'internal/execution/native_residual_ffn.py',
        'internal/execution/native_linear_split.py', 'internal/execution/native_split.py',
        'internal/triton_kernels.py', 'lora.py', '_kernels/_gemm.abi3.so',
    }
    for name, expected in payload.runtime_modules.items():
        assert digest(base / name) == expected, name
    base = Path(glue.__file__).resolve().parent
    assert set(payload.package_modules) == {'__init__.py', 'glue.py', 'adaln_pruned.py', 'overlay.py'}
    for name, expected in payload.package_modules.items():
        assert digest(base / name) == expected, name
    assert payload.glue_identities == {s.name: s.identity for s in (glue.GLUE, glue.GLUE_FP8)}
    assert version('tensorfs') == payload.tensorfs_version
    assert digest(_ext.__file__) == payload.extension_sha256
    assert digest(Path(tensorfs.__file__).resolve().parent / '_varena_torch.so') == payload.shim_sha256
    assert str(torch.__version__) == '2.14.0+cu130'
    assert torch.version.git_version == '08187d9e0fba026dc8217405802ab5381dc88d90'
    assert ctx.device.type == 'cuda' and torch.cuda.device_count() == 1
    assert torch.cuda.get_device_capability(0) == (8, 9)
    actual_uuid = str(torch.cuda.get_device_properties(0).uuid)
    assert actual_uuid.removeprefix('GPU-').lower() == payload.gpu_uuid.removeprefix('GPU-').lower()
    request = _REQUEST.get()
    assert isinstance(request, Request) and request.id == ctx.request_id and not request.idle
    assert request in request.memory.requests and not request.memory.models
    assert request.memory.budget_bytes is None
    return request, vt, {**msgspec.to_builtins(payload), 'pid': os.getpid(),
                         'request_id': ctx.request_id, 'gpu_uuid': actual_uuid}


def trim(vt, device):
    torch.cuda.synchronize(device)
    vt.trim(device)
    torch.cuda.synchronize(device)
    vt.trim(device)


def menu(native, device, rows, chunk):
    """Independent ABI query for both projections and actual full/final row counts."""
    tail = rows % chunk or chunk
    queries = {name: native.int8_plan(device, False, count, n, k, n)
               for name, count, n, k in (
                   ('up', chunk, 2 * INNER, WIDTH), ('up_tail', tail, 2 * INNER, WIDTH),
                   ('down', chunk, WIDTH, INNER), ('down_tail', tail, WIDTH, INNER))}
    assert all(info['status'] == 0 for info in queries.values())
    gemm = [(count, k) for name, count, k in (
        ('up', chunk, WIDTH), ('up_tail', tail, WIDTH),
        ('down', chunk, INNER), ('down_tail', tail, INNER)) if queries[name]['kind'] == 0]
    allocations = [2 * chunk * WIDTH, 4 * chunk * INNER, 2 * chunk * INNER, 2 * chunk * WIDTH]
    if gemm:
        allocations += [max(m * k for m, k in gemm), 4 * max(m for m, _ in gemm)]
    workspace = max(q['workspace_bytes'] for q in queries.values())
    if workspace:
        allocations.append(workspace)
    return allocations, {'full_rows': chunk, 'tail_rows': tail, 'queries': queries,
                         'workspace_bytes': workspace, 'allocations': allocations}


def make_block(on, generator, launch):
    from cozy_runtime import lora
    from cozy_runtime.internal.encoding.leaves import Int8ConvRotNativeLeaf
    from diffusers.models.attention import FeedForward

    block = torch.nn.Module()
    with torch.device('meta'):
        block.ff = FeedForward(WIDTH, dim_out=WIDTH, inner_dim=INNER,
                               activation_fn='swiglu', bias=False)
    block.norm2 = torch.nn.RMSNorm(WIDTH, eps=1e-6, dtype=torch.bfloat16, device=on)
    block._glue, block._fusion_ff_fp8 = launch, False
    block._residual_ffn_entries = (
        next(e for e in glue.GLUE.entries if e.kernel is glue._norm_mod_kernel
             and e.constexprs['RESIDUAL'] == 1),
        next(e for e in glue.GLUE.entries if e.kernel is glue._swiglu_kernel),
        next(e for e in glue.GLUE.entries if e.kernel is glue._gate_add_kernel),
    )
    reads = [0]

    def armed():
        reads[0] += 1

    for parent, name, n, k in ((block.ff.net[0], 'proj', 2 * INNER, WIDTH),
                                (block.ff.net, '2', WIDTH, INNER)):
        data = torch.randint(-127, 128, (n, k), generator=generator,
                             dtype=torch.int8).to(on)
        scales = torch.full((n,), 0.001, dtype=torch.float32, device=on)
        old = getattr(parent, name)
        leaf = Int8ConvRotNativeLeaf().leaf(
            torch, {'weight': data, 'scale': scales}, old, torch.bfloat16)
        hook = lora.Hook(armed)
        hook._cozy_baked = True
        leaf.register_forward_hook(hook)
        setattr(parent, name, leaf)
    act = block.ff.net[0]
    act.__class__ = glue.FusedSwiGLU
    act._glue, act._fusion_fp8 = launch, False
    return block, reads


def query_proof(native, library, leaf, source, reference, counts):
    """Native automatic dispatch, independent of int8_plan's returned configuration."""
    from cozy_runtime.internal import prebuilt, verify

    records = []
    device, columns, channels = source.device.index or 0, source.shape[-1], reference.shape[-1]
    for count in counts:
        values = source.reshape(-1, columns)[:count]
        expected = reference.reshape(-1, channels)[:count]
        target = torch.empty((count, channels), dtype=source.dtype, device=source.device)
        info = native.int8_plan(device, False, count, channels, columns, channels)
        assert info['status'] == 0
        actual_workspace = []

        def allocate(size, actual_workspace=actual_workspace):
            actual_workspace.append(size)
            return torch.empty(size, dtype=torch.uint8, device=source.device)

        stream = int(torch.cuda.current_stream(device).cuda_stream)
        if info['kind'] == 1:
            code = native.gemv(device, values.data_ptr(), False, leaf.data.data_ptr(),
                               leaf.scale.data_ptr(), 0, target.data_ptr(), False,
                               count, channels, columns, stream)
        else:
            quantized, row_scale = library.int8_quantize(values)
            code = native.gemm(device, 0, False, quantized.data_ptr(), row_scale.data_ptr(),
                               leaf.data.data_ptr(), leaf.scale.data_ptr(), 0,
                               target.data_ptr(), channels, False, count, channels, columns,
                               -1, allocate, stream)
            del quantized, row_scale
        library._check(code, 'actual residual FFN automatic workspace')
        assert actual_workspace == ([info['workspace_bytes']] if info['workspace_bytes'] else [])
        relative = verify.error(torch, target, expected)
        assert relative <= prebuilt.BUDGET
        records.append({'rows': count, 'channels': channels, 'columns': columns, 'query': info,
                        'actual_workspace': actual_workspace, 'relative_l2': relative})
        del values, expected, target, allocate
    return records


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Report:
    from cozy_runtime import _kernels
    from cozy_runtime.internal import int8_linear, prebuilt, triton_kernels, verify
    from cozy_runtime.internal.execution.native_residual_ffn import split_residual_ffn

    request, vt, provenance = actual_request(payload, ctx)
    device = ctx.device.index or 0
    on = torch.device(ctx.device.type, device)
    torch.set_num_threads(1)
    assert int8_linear.native(torch, device) is not None
    library = int8_linear.library(device)
    assert library.family == 'mma.sync'
    selected_set = glue.kernels(on)
    launch = triton_kernels.load(selected_set, device=on)
    assert type(launch).__name__ == '_Launcher'
    provenance['launcher_key'] = launch.key
    provenance['loaded_set'] = selected_set.name
    gc.collect()
    trim(vt, device)
    initial = vt.stats(device)
    generator = torch.Generator().manual_seed(payload.seed)
    cases, keys = [], []
    with torch.inference_mode():
        block, hook_reads = make_block(on, generator, launch)
        residual = torch.randn((1, ROWS, WIDTH), generator=generator,
                               dtype=torch.bfloat16).to(on)
        attention = torch.randn((1, ROWS, WIDTH), generator=generator,
                                dtype=torch.bfloat16).to(on)
        tables = tuple((torch.randn((4, WIDTH), generator=generator,
                                    dtype=torch.bfloat16) * .1).to(on) for _ in range(4))
        index = (torch.arange(ROWS, dtype=torch.int64) % 4).to(on)
        marker = torch.zeros(1, dtype=torch.int64, device=on)
        tail = glue._Tail(block, residual, index, *tables)
        intent = tail.intent()
        reference_gpu = attention.clone()
        tail.run(tail, reference_gpu)
        reference = reference_gpu.cpu()
        del reference_gpu
        assert hook_reads[0] == 0
        # These are the original package wrappers and normal projection module calls.
        scratch_attention = attention.clone()
        hidden, normed = glue.gate_add_rmsnorm_modulate(
            launch, residual, tables[0], scratch_attention, index, block.norm2.weight,
            tables[1], tables[2], block.norm2.eps, hidden_out=scratch_attention)
        packed = block.ff.net[0].proj(normed)
        activation = glue.swiglu(launch, packed)
        projected = block.ff.net[2](activation)
        counts = (ROWS, 4098, 4097, 2048, 3, 2, 1)
        query_execution = {
            'up': query_proof(_kernels._gemm, library, block.ff.net[0].proj, normed, packed, counts),
            'down': query_proof(_kernels._gemm, library, block.ff.net[2], activation, projected, counts),
        }
        del scratch_attention, hidden, normed, packed, activation, projected
        queried = {chunk: menu(_kernels._gemm, device, ROWS, chunk)
                   for chunk in (ROWS, 4098, 2048, 2)}
        assert queried[ROWS][1]['workspace_bytes'] > 0
        held = {}
        for chunk in (ROWS, 4098, 2048, None):
            output = attention.clone()
            held.clear()
            capacity_key = f'fixture.residual-ffn-capacity/{chunk}'

            def step(i, marker, residual, index, *tables, chunk=chunk, output=output,
                     block=block, capacity_key=capacity_key):
                del i, marker
                current = glue._Tail(block, residual, index, *tables)
                trim(vt, device)
                before = vt.stats(device)
                allocations, _ = queried[chunk or 2]
                exact = vt.split_select(device, capacity_key, [('target', allocations)],
                                        preferred='target',
                                        stream=int(torch.cuda.current_stream(device).cuda_stream))
                if chunk is None:
                    assert exact['required_bytes'] > 0, (
                        'fixture cannot force minimum refusal with fully reusable scratch; '
                        'stop before the operation instead of executing tiny chunks')
                vt.set_limit(device, before['mapped_bytes'] + exact['required_bytes']
                             - (1 if chunk is None else 0))
                try:
                    result = split(current.run, current, Cut(1), out=output, intent=current.intent())
                    assert result is output
                    held['accepted'] = True
                except torch.OutOfMemoryError as error:
                    if chunk is not None:
                        raise
                    after = vt.stats(device)
                    assert error.varena_no_retry and error.required_bytes > error.available_bytes
                    assert after['allocations'] == before['allocations']
                    held['refusal'] = {'required_bytes': error.required_bytes,
                                       'available_bytes': error.available_bytes,
                                       'allocation_delta': 0, 'no_retry': True}
                finally:
                    vt.set_limit(device, None)

            try:
                denoise_loop(step, range(1), state=(marker,), inputs=(residual, index, *tables),
                             key=('actual-h3-residual-ffn', ROWS, WIDTH, INNER))
            finally:
                vt.phase_forget(device, capacity_key)
            phase = request.working_phases[-1]
            actual = output.cpu()
            if chunk is None:
                assert 'accepted' not in held and 'refusal' in held
                assert torch.equal(actual, attention.cpu()), 'minimum refusal wrote output'
                assert phase['invalidated'] and not phase['published']
                cases.append({'refusal': held['refusal'], 'output_unchanged': True, 'phase': phase})
            else:
                relative = verify.error(torch, actual, reference)
                assert torch.isfinite(actual).all() and relative <= prebuilt.BUDGET
                observations = phase['split_observations']
                assert len(observations) == 1 and observations[0]['published'], observations
                assert phase['cut_choices'] == 1
                sample = observations[0]
                selected = request.working_splits[sample['key']]
                assert selected['row_chunk'] == chunk and selected['site'] == 'residual-ffn-tail'
                keys.append(sample['key'])
                cases.append({'rows': ROWS, 'row_chunk': chunk, 'phase': phase,
                              'selection': json.loads(json.dumps(selected)),
                              'relative_l2': relative, 'budget': prebuilt.BUDGET,
                              'exact': torch.equal(actual, reference), 'query': queried[chunk][1]})
            del actual, output, step

        # Unsupported contracts return before writes; no original-call fallback is run here.
        output = attention.clone()
        declined = []
        hook = next(iter(block.ff.net[2]._forward_hooks.values()))
        hook._cozy_baked = False
        assert split_residual_ffn(intent, output, (Cut(1),)) is None
        declined.append('unbaked_hook')
        hook._cozy_baked = True
        for name, declared, target in (
            ('nonexclusive', replace(intent, exclusive_out=False), output),
            ('output_alias', intent, residual),
            ('unaligned_index', replace(intent, index=torch.empty(ROWS + 1, dtype=torch.int64,
                                                                device=on)[1:]), output)):
            assert split_residual_ffn(declared, target, (Cut(1),)) is None
            declined.append(name)
        assert torch.equal(output.cpu(), attention.cpu())
        del output, hook, declared, target
        small = glue._Tail(block, residual[:, :1], index[:1], *tables)
        one_reference = attention[:, :1].clone()
        small.run(small, one_reference)
        one_reference = one_reference.cpu()
        output = attention[:, :1].clone()

        def single(i, marker, *, small=small, output=output):
            del i, marker
            assert split(small.run, small, Cut(1), out=output, intent=small.intent()) is output

        denoise_loop(single, range(1), state=(marker,), key=('actual-h3-residual-ffn', 1))
        phase = request.working_phases[-1]
        observations = phase['split_observations']
        assert len(observations) == 1 and observations[0]['published']
        sample = observations[0]
        selected = request.working_splits[sample['key']]
        assert selected['row_chunk'] == 1
        relative = verify.error(torch, output.cpu(), one_reference)
        assert relative <= prebuilt.BUDGET and hook_reads[0] == 0
        keys.append(sample['key'])
        cases.append({'rows': 1, 'row_chunk': 1, 'relative_l2': relative, 'phase': phase,
                      'selection': json.loads(json.dumps(selected))})
        assert set(request.working_phase_keys) == set(keys)
        del single, output, small, intent, tail, block, residual, attention, tables, index, marker
        held.clear()
    gc.collect()
    trim(vt, device)
    final = vt.stats(device)
    assert final['allocated_bytes'] == initial['allocated_bytes'], (initial, final)
    assert final['pending_bytes'] == 0 and final['pools'] == initial['pools']
    return Report(True, json.dumps({'passed': True, 'provenance': provenance, 'cases': cases,
                                   'query_execution': query_execution,
                                   'declined_before_write': declined, 'hook_factor_reads': hook_reads[0],
                                   'initial': initial, 'final': final,
                                   'request_owned_keys': sorted(set(keys))}, sort_keys=True))


@app.entrypoint(accelerator=True)
def cleanup(payload: Cleanup, ctx: Context) -> Report:
    request, vt, provenance = actual_request(payload, ctx)
    assert os.getpid() == payload.expected_pid and not request.working_phase_keys
    removed = {key: vt.phase_forget(ctx.device.index or 0, key) for key in payload.keys}
    assert removed and not any(removed.values()), removed
    return Report(True, json.dumps({'passed': True, 'provenance': provenance,
                                   'prior_request_keys_already_retired': removed}, sort_keys=True))
