"""Actual CUDA duplicate-export binding refusal across all three VAE edge contracts."""
from __future__ import annotations

import gc
import hashlib
import json
import os
import traceback
from importlib.metadata import version
from pathlib import Path

import msgspec
import torch
from cozy_runtime.author import App, Context, denoise_loop

app = App()
MODULES = (
    '_kernels/_dense.abi3.so', '_kernels/__init__.py', 'internal/dense_linear.py',
    'internal/execution/native_vae_prefix.py', 'internal/execution/native_vae_post_quant.py',
    'internal/execution/native_vae_output.py', 'internal/execution/native_vae.py',
    'internal/execution/phase.py', 'internal/execution/catch.py', 'internal/residency.py',
    'graphs/domain.py', 'internal/accel.py', 'internal/int8_linear.py',
    'internal/prebuilt.py', 'internal/kernel_ready.py', '_kernels/_gemm.abi3.so',
)


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    runtime_version: str
    runtime_modules: dict[str, str]
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    gpu_uuid: str
    seed: int = 1731


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
    assert set(payload.runtime_modules) == set(MODULES)
    assert all(digest(root / name) == expected for name, expected in payload.runtime_modules.items())
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
                         'request_id': ctx.request_id, 'gpu_uuid': uuid,
                         'diffusers_version': version('diffusers')}


def settle(vt, device):
    gc.collect()
    stream = torch.cuda.current_stream(device)
    assert stream.cuda_stream > 2
    stream.synchronize()
    vt.trim(device)
    stream.synchronize()
    vt.trim(device)



def inputs(kind, device):
    if kind == 'post_quant':
        module = torch.nn.Conv3d(24, 24, 1, device=device, dtype=torch.float32).eval().requires_grad_(False)
        return (module,), torch.full((1, 24, 7, 16, 16), 7, dtype=torch.float16, device=device)
    if kind == 'output':
        norm = torch.nn.LayerNorm(2048, eps=1e-5, device=device, dtype=torch.float32).eval().requires_grad_(False)
        projection = torch.nn.Linear(2048, 3072, device=device, dtype=torch.float32).eval().requires_grad_(False)
        return (norm, projection), torch.full((1, 1797, 2048), 7, dtype=torch.float32, device=device)
    from diffusers.models.autoencoders.autoencoder_kl_minimax_h3 import (
        MiniMaxH3VideoViTDecoder3d, MiniMaxH3VideoRotaryPosEmbed,
    )
    with torch.device('meta'):
        decoder = MiniMaxH3VideoViTDecoder3d()
    decoder.eval().requires_grad_(False)
    decoder.proj_in.to(dtype=torch.float32).to_empty(device=device)
    decoder.register_tokens = torch.nn.Parameter(torch.zeros(1, 4, 2048, device=device, dtype=torch.float32), requires_grad=False)
    decoder.rope = MiniMaxH3VideoRotaryPosEmbed(48).to(device).eval().requires_grad_(False)
    return (decoder,), torch.full((1, 24, 7, 16, 16), 7, dtype=torch.float16, device=device)


def same_baseline(before, after):
    for name in ('allocated_bytes', 'allocated_capacity', 'mapped_bytes', 'pending_bytes',
                 'pools', 'pools_bytes', 'event_handles'):
        assert before[name] == after[name], (name, before[name], after[name])


def exercise(request, vt, device):
    from cozy_runtime._kernels import _dense
    from cozy_runtime.internal import dense_linear
    from cozy_runtime.internal.execution import (
        native_vae_post_quant, native_vae_output, native_vae_prefix, phase,
    )
    stream = torch.cuda.current_stream(device)
    on = request.memory.device
    result = {'passed': False, 'cases': [], 'keys': [], 'operation_execute_calls': 0,
              'gpu_numerical_qualification': False, 'vendor_allocation_closure': 'not established by binding refusal'}
    owner = None
    settle(vt, device)
    before = vt.stats(device)

    def one(kind, consumer):
        modules, value = inputs(kind, on)
        if kind == 'post_quant':
            description = consumer.describe(*modules, value)
        else:
            description = consumer.describe(*modules, value, geometry=(7, 16, 16))
        assert description is not None
        prepared = consumer.prepare(description, context=owner, stream=stream)
        assert prepared is not None
        allocations = [size for _, size in prepared.allocations]
        key = 'fixture.vae_span/1:' + kind
        request.memory.remember_working_phase(key, (request,))
        result['keys'].append(key)
        marker = torch.zeros(1, dtype=torch.int64, device=on)
        held = {}

        def step(i, marker, x):
            del i, marker
            selected = vt.split_select(device, key, [('whole', allocations)],
                                       preferred='whole', stream=stream.cuda_stream)
            token = phase.begin_split(device, key, 'whole', selected, stream=stream.cuda_stream)
            try:
                buffers = consumer.Buffers.allocate(description, prepared.info['workspace_bytes'])
                if kind == 'post_quant':
                    original = x
                    duplicate = torch.from_dlpack(original)
                    buffers.result = target = duplicate
                elif kind == 'output':
                    original = buffers.dense_result
                    original.fill_(7)  # Fixture initialization, before the attempted consumer bind.
                    duplicate = torch.from_dlpack(original)
                    target = duplicate.flatten()[:description.batch * 1792 * 3072].view(description.result_shape)
                    buffers.result = target
                else:
                    original = buffers['cosine']
                    original.fill_(7)
                    duplicate = torch.from_dlpack(original)
                    buffers.values['sine'] = target = duplicate
                assert original.is_contiguous() and target.is_contiguous()
                assert original.device == target.device and original.device.type == 'cuda'
                assert not torch._C._is_alias_of(original, target), 'export did not create the distinct-Storage counterexample'
                assert original.untyped_storage()._cdata != target.untyped_storage()._cdata
                assert original.data_ptr() == target.data_ptr()
                held.update(original=original, before=original.cpu(),
                            address=original.data_ptr(), original_bytes=original.numel() * original.element_size(),
                            alias_bytes=target.numel() * target.element_size(),
                            storage_identity_alias=False, export='torch.from_dlpack(actual CUDA Tensor)')
                try:
                    prepared.bind(x, buffers)
                    raise AssertionError('overlapping CUDA exports were accepted')
                except ValueError as error:
                    held['error'] = str(error)
                    assert 'independent owned storage' in str(error)
            finally:
                held['submission'] = vt.split_end(device, token, submitted=False)

        denoise_loop(step, range(1), state=(marker,), inputs=(value,), key=('vae-span', kind))
        original = held.pop('original')
        expected = held.pop('before')
        assert torch.equal(original.cpu(), expected), 'binding mutated the aliased bytes'
        assert not held['submission']['published']
        result['cases'].append({'kind': kind, 'passed': True, 'bytes_unchanged': True,
                                'bind_refused_before_execute': True, 'provider': prepared.info,
                                'phase': request.working_phases[-1], **held})
        del original, expected, marker, modules, value, description, prepared, step
        held.clear()
        settle(vt, device)

    try:
        with torch.no_grad(), torch.autocast('cuda', dtype=torch.float16):
            owner = dense_linear.checked(_dense.Context, device, stream.cuda_stream)
            for kind, consumer in (('post_quant', native_vae_post_quant), ('output', native_vae_output),
                                   ('prefix', native_vae_prefix)):
                one(kind, consumer)
        assert len(result['cases']) == 3
        result['passed'] = True
    except Exception:
        result['error'] = traceback.format_exc()
    finally:
        try:
            settle(vt, device)
            if owner is not None:
                dense_linear.checked(owner.close, stream.cuda_stream)
            owner = None
            settle(vt, device)
            result.update(before=before, after=vt.stats(device))
            same_baseline(before, result['after'])
        except Exception:
            result['passed'] = False
            result['cleanup_error'] = traceback.format_exc()
    return result


@app.entrypoint(accelerator=True)
def spans(payload: Probe, ctx: Context) -> Report:
    from cozy_runtime.graphs.domain import Domain
    from cozy_runtime.internal import int8_linear
    torch.set_num_threads(1)
    request, vt, provenance = actual(payload, ctx)
    device = ctx.device.index or 0
    startup = vt.stats(device)
    assert int8_linear.native(torch, device) is not None
    ready = torch.cuda.Event()
    ready.record(torch.cuda.current_stream(device))
    ready.synchronize()
    vt.trim(device)
    before = vt.stats(device)
    domain = Domain(torch, request.memory)
    result = {'passed': False}
    try:
        domain.initialize()
        domain.stream.wait_event(ready)
        with torch.cuda.stream(domain.stream), domain.using():
            result = exercise(request, vt, device)
    except Exception:
        result['error'] = traceback.format_exc()
    finally:
        result.update(provenance=provenance, startup_before=startup, pre_domain=before)
        try:
            domain.finish()
            vt.trim(device)
            result['post_domain'] = vt.stats(device)
            same_baseline(before, result['post_domain'])
        except Exception:
            result['passed'] = False
            result['domain_cleanup_error'] = traceback.format_exc()
    return Report(result['passed'], json.dumps(result, sort_keys=True))


@app.entrypoint(accelerator=True)
def cleanup(payload: Cleanup, ctx: Context) -> Report:
    request, vt, provenance = actual(payload, ctx)
    assert os.getpid() == payload.expected_pid and not request.working_phase_keys
    removed = {key: vt.phase_forget(ctx.device.index or 0, key) for key in payload.keys}
    assert removed and not any(removed.values()), removed
    return Report(True, json.dumps({'passed': True, 'provenance': provenance,
                                   'prior_request_keys_already_retired': removed}, sort_keys=True))
