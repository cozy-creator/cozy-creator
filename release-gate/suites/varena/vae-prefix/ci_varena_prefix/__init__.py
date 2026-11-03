"""Ordinary Cozy child client for actual H3 input-prefix provider qualification."""
from __future__ import annotations

import gc
import hashlib
import inspect
import json
import os
import traceback
from collections import Counter
from dataclasses import asdict
from importlib.metadata import version
from pathlib import Path

import msgspec
import torch
from cozy_runtime.author import App, Context, denoise_loop
from torch.utils._python_dispatch import TorchDispatchMode

app = App()
MODULES = (
    '_kernels/_dense.abi3.so', '_kernels/__init__.py', 'internal/dense_linear.py',
    'internal/execution/native_vae_prefix.py', 'internal/execution/native_vae.py',
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


class Census(TorchDispatchMode):
    def __init__(self, vt, device):
        super().__init__()
        self.vt, self.device, self.calls = vt, device, []
        self.inference, self.grad = torch.is_inference_mode_enabled(), torch.is_grad_enabled()

    def __torch_dispatch__(self, func, types, args=(), kwargs=None):
        before = self.vt.stats(self.device)
        with torch.inference_mode(self.inference), torch.set_grad_enabled(self.grad):
            result = func(*args, **(kwargs or {}))
        after = self.vt.stats(self.device)
        self.calls.append({'op': str(func), 'allocations': after['allocations'] - before['allocations'],
                           'live_delta': after['allocated_bytes'] - before['allocated_bytes']})
        return result


def construction(device):
    from diffusers.models.autoencoders.autoencoder_kl_minimax_h3 import (
        MiniMaxH3VideoViTDecoder3d, MiniMaxH3VideoRotaryPosEmbed,
    )
    with torch.device('meta'):
        decoder = MiniMaxH3VideoViTDecoder3d()
    decoder.eval().requires_grad_(False)
    decoder.proj_in.to_empty(device=device)
    decoder.register_tokens = torch.nn.Parameter(torch.empty(1, 4, 2048, device=device), requires_grad=False)
    decoder.rope = MiniMaxH3VideoRotaryPosEmbed(48).to(device).eval().requires_grad_(False)
    assert decoder.proj_in.weight.shape == (2048, 24) and decoder.num_register_tokens == 4
    assert decoder.rope.inv_freq.shape == (8,) and decoder.rope.inv_freq.dtype is torch.float32
    return decoder, {'decoder': hashlib.sha256(inspect.getsource(MiniMaxH3VideoViTDecoder3d).encode()).hexdigest(),
                     'rope': hashlib.sha256(inspect.getsource(MiniMaxH3VideoRotaryPosEmbed).encode()).hexdigest()}


def reference(decoder, value):
    batch = value.shape[0]
    projected = decoder.proj_in(value.permute(0, 2, 3, 4, 1).reshape(batch, 1792, 24))
    registers = decoder.register_tokens.expand(batch, -1, -1).to(projected.dtype)
    hidden = torch.cat((projected, registers, torch.zeros_like(projected[:, :1])), dim=1)
    grids = [2.0 * (torch.arange(0.5, size, dtype=torch.float32, device=value.device) / size) - 1.0
             for size in (7, 16, 16)]
    positions = torch.stack(torch.meshgrid(*grids, indexing='ij'), dim=-1).flatten(0, 2)[None]
    suffix = positions.new_zeros(1, 5, 3)
    cosine, sine = decoder.rope(torch.cat((positions, suffix), dim=1))
    return hidden, (cosine, sine)


def cpu_result(result):
    hidden, (cosine, sine) = result
    return hidden.cpu(), (cosine.cpu(), sine.cpu())


def compare(observed, expected):
    hidden, (cosine, sine) = observed
    target, (target_cosine, target_sine) = expected
    torch.testing.assert_close(hidden, target, rtol=0.002, atol=0.0001)
    torch.testing.assert_close(hidden[:, 1792:], target[:, 1792:], rtol=0, atol=0)
    torch.testing.assert_close(cosine, target_cosine, rtol=0, atol=0)
    torch.testing.assert_close(sine, target_sine, rtol=0, atol=0)
    delta = hidden[:, :1792].float() - target[:, :1792].float()
    error_norm = float(torch.linalg.vector_norm(delta))
    reference_norm = float(torch.linalg.vector_norm(target[:, :1792].float()))
    relative_l2 = error_norm / reference_norm if reference_norm else (0.0 if error_norm == 0 else float('inf'))
    assert relative_l2 <= 0.001
    return {'projection_max_abs': float(delta.abs().max()), 'projection_relative_l2': relative_l2,
            'projection_bit_exact': torch.equal(hidden[:, :1792], target[:, :1792]),
            'register_zero_and_rope_bit_exact': True,
            'projection_bounds': {'rtol': 0.002, 'atol': 0.0001, 'relative_l2': 0.001}}


def same_baseline(before, after):
    for name in ('allocated_bytes', 'allocated_capacity', 'mapped_bytes', 'pending_bytes',
                 'pools', 'pools_bytes', 'event_handles'):
        assert after[name] == before[name], (name, before[name], after[name])


def exercise(payload, ctx, request, vt):
    from cozy_runtime._kernels import _dense
    from cozy_runtime.internal import dense_linear
    from cozy_runtime.internal.execution import native_vae_prefix as consumer, phase

    device, on = ctx.device.index or 0, request.memory.device
    stream = torch.cuda.current_stream(device)
    result = {'passed': False, 'cases': [], 'keys': [], 'projection_alternative': 'fused-bias Lt replaces strided matmul plus separate FP16 bias',
              'library_internal_allocation_closure': 'unproven; needs independent driver census',
              'outer_decoder_qualified': False, 'model_quality_qualified': False,
              'reference_linear_flatten_environment': os.environ.get('TORCH_LINEAR_FLATTEN_3D')}
    owner = None
    owned = []
    settle(vt, device)
    initial = result['initial'] = vt.stats(device)

    def run():
        nonlocal owner
        owner = dense_linear.checked(_dense.Context, device, stream.cuda_stream)
        decoder, result['upstream_class_sha256'] = construction(on)
        projection = decoder.proj_in
        marker = torch.zeros(1, dtype=torch.int64, device=on)
        generator = torch.Generator().manual_seed(payload.seed)
        with torch.no_grad(), torch.autocast('cuda', dtype=torch.float16):
            decoder.register_tokens.copy_(torch.randn(1, 4, 2048, generator=generator))
            for weight_dtype in (torch.float16, torch.float32):
                projection.weight.data = projection.weight.data.to(weight_dtype)
                projection.weight.copy_(torch.randn(projection.weight.shape, generator=generator) * 0.02)
                projection.bias.copy_(torch.rand(projection.bias.shape, generator=generator) * 0.1 - 0.05)
                assert projection.bias.dtype is torch.float32
                for batch in (1, 3, 7):
                    value = (torch.randn(batch, 24, 7, 16, 16, generator=generator) * 0.125).to(on, torch.float16)
                    expected = cpu_result(reference(decoder, value))
                    description = consumer.describe(decoder, value, geometry=(7, 16, 16))
                    assert description is not None
                    census = Census(vt, device)
                    with census:
                        prepared = consumer.prepare(description, context=owner, stream=stream)
                    assert prepared is not None
                    facts = prepared.facts()
                    assert facts['preparation_allocations'] == (('bias', 4096),)
                    assert Counter(c['live_delta'] for c in census.calls if c['allocations']) == Counter([4096])
                    assert sum(c['allocations'] for c in census.calls) == 1
                    info = facts['provider']
                    assert info['workspace_bytes'] == max(info['heuristic_workspace_bytes'], info['checked_workspace_bytes'])
                    allocations = [size for _, size in prepared.allocations]
                    semantic = json.dumps((facts, asdict(prepared.policy)), sort_keys=True)
                    key = 'fixture.prefix/1:' + hashlib.sha256(semantic.encode()).hexdigest()
                    request.memory.remember_working_phase(key, (request,))
                    result['keys'].append(key)
                    held = {}

                    def step(i, marker, x, *, late=False):
                        del i, marker
                        selected = vt.split_select(device, key, [('whole', allocations)],
                                                   preferred='whole', stream=stream.cuda_stream)
                        token = phase.begin_split(device, key, 'whole', selected, stream=stream.cuda_stream)
                        accepted = False
                        bound = buffers = None
                        census = Census(vt, device)
                        try:
                            with census:
                                buffers = consumer.Buffers.allocate(description, info['workspace_bytes'])
                                bound = prepared.bind(x, buffers)
                                owned.append(bound)
                                held['bound'] = bound
                                held['execute_calls'] = held.get('execute_calls', 0) + 1
                                before = vt.stats(device)
                                output = bound.execute()
                                after = vt.stats(device)
                                held.update(execute_before=before, execute_after=after)
                                bound.wait()
                                assert bound.poll() and bound.completed and not bound.retained
                                held['output'] = output
                                if late:
                                    # A real managed allocation refusal after actual output.
                                    cap = after['mapped_bytes'] + request.memory.native.stats()['resident_bytes']
                                    held['injected_cap'] = cap
                                    vt.set_limit(device, cap)
                                    try:
                                        torch.empty(cap + 2 * 1024 * 1024, dtype=torch.uint8, device=on)
                                        raise AssertionError('post-write allocation unexpectedly fit')
                                    finally:
                                        vt.set_limit(device, None)
                            accepted = True
                            held['selection'] = selected
                        except BaseException as error:
                            error.varena_no_retry = True
                            raise
                        finally:
                            # On a real submit failure, keep the exact Bound in held through
                            # a checked scoped completion; do not drop accepted source owners.
                            try:
                                if bound is not None and bound.submitted and not bound.completed:
                                    bound.wait()
                            finally:
                                held['submission'] = vt.split_end(device, token, submitted=accepted)
                                held['census'] = census.calls

                    denoise_loop(step, range(1), state=(marker,), inputs=(value,), key=('input-prefix', key))
                    bound = held.pop('bound')
                    output = held.pop('output')
                    assert torch.equal(bound.buffers['hidden'][:, :1792],
                                       bound.buffers['dense_result'].view(batch, 1792, 2048))
                    observed = cpu_result(output)
                    numerical = compare(observed, expected)
                    assert tuple(observed[0].shape) == (batch, 1797, 2048) and observed[0].dtype is torch.float16
                    case = {'case': 'actual-h3-prefix', 'facts': facts,
                        'policy': asdict(prepared.policy), 'preparation_census': census.calls,
                        **numerical, 'packing_copy_bit_exact': True,
                        'phase': request.working_phases[-1], **held}
                    result['cases'].append(case)
                    submission = held['submission']
                    completed = [row for row in case['phase']['split_observations']
                                 if all(row[field] == submission[field]
                                        for field in ('observation_id', 'key', 'choice_id'))]
                    case['completion_observations'] = completed
                    assert held['execute_calls'] == 1
                    assert submission['pending'] and not submission['published']
                    assert len(completed) == 1
                    assert completed[0]['published'] and not completed[0]['pending'] and not completed[0]['reason']
                    assert held['execute_after']['allocations'] == held['execute_before']['allocations']
                    assert Counter(c['live_delta'] for c in held['census'] if c['allocations']) == Counter(allocations)
                    assert sum(c['allocations'] for c in held['census']) == len(allocations)
                    try:
                        bound.execute()
                        raise AssertionError('one-shot replay accepted')
                    except RuntimeError as replay:
                        assert 'one-shot' in str(replay)
                    owned[:] = [item for item in owned if item is not bound]
                    bound = output = observed = None
                    held.clear()
                    settle(vt, device)
                    if batch == 1:
                        # Before-write refusal of an actual aliasing caller allocation.
                        alias_prepared = consumer.prepare(description, context=owner, stream=stream)
                        alias_buffers = consumer.Buffers.allocate(description, alias_prepared.info['workspace_bytes'])
                        alias_buffers.values['sine'] = alias_buffers['cosine']
                        alias_buffers['cosine'].fill_(-13)
                        failure = None
                        try:
                            alias_prepared.bind(value, alias_buffers)
                        except ValueError as error:
                            failure = str(error)
                        assert failure and 'independent owned storage' in failure
                        assert bool((alias_buffers['cosine'] == -13).all())
                        result['cases'].append({'case': 'alias-refusal-before-write', 'error': failure})
                        alias_prepared = alias_buffers = None

                        # The native Context itself refuses teardown with a retained real Plan.
                        try:
                            dense_linear.checked(owner.close, stream.cuda_stream)
                            raise AssertionError('retained provider context closed')
                        except ValueError as error:
                            assert 'retained plans' in str(error)
                            result['cases'].append({'case': 'retained-context-refusal', 'error': str(error)})

                        refused = {}
                        def minimum(i, marker, x):
                            del i, marker, x
                            settle(vt, device)
                            probe = vt.split_select(device, key, [('whole', allocations)], preferred='whole', stream=stream.cuda_stream)
                            assert probe['required_bytes'] > 0
                            before = vt.stats(device)
                            cap = before['mapped_bytes'] + request.memory.native.stats()['resident_bytes'] + probe['required_bytes'] - 1
                            vt.set_limit(device, cap)
                            try:
                                vt.split_select(device, key, [('whole', allocations)], preferred='whole', stream=stream.cuda_stream)
                            except torch.OutOfMemoryError as error:
                                refused['error'] = str(error)
                            finally:
                                vt.set_limit(device, None)
                            refused.update(before=before, after=vt.stats(device), cap=cap, probe=probe)
                        denoise_loop(minimum, range(1), state=(marker,), inputs=(value,), key=('input-prefix-min', key))
                        assert refused.get('error') and refused['before']['allocations'] == refused['after']['allocations']
                        result['cases'].append({'case': 'managed-minimum-before-write', **refused})

                        prepared = consumer.prepare(description, context=owner, stream=stream)
                        assert prepared is not None
                        assert {k: v for k, v in prepared.info.items() if k != 'bias_address'} == {
                            k: v for k, v in info.items() if k != 'bias_address'}
                        failure = None
                        try:
                            denoise_loop(lambda i, marker, x: step(i, marker, x, late=True), range(1),
                                         state=(marker,), inputs=(value,), key=('input-prefix-late', key))
                        except torch.OutOfMemoryError as error:
                            failure = str(error)
                            assert getattr(error, 'varena_no_retry', False)
                        assert failure and held['execute_calls'] == 1 and not held['submission']['published']
                        bound = held.pop('bound')
                        output = held.pop('output')
                        compare(cpu_result(output), expected)
                        assert bound.completed and not bound.retained
                        result['cases'].append({'case': 'post-write-refusal-once', 'error': failure,
                                               'phase': request.working_phases[-1], **held})
                        late_case = result['cases'][-1]
                        late_submission = held['submission']
                        late_completed = [row for row in late_case['phase']['split_observations']
                                          if all(row[field] == late_submission[field]
                                                 for field in ('observation_id', 'key', 'choice_id'))]
                        late_case['completion_observations'] = late_completed
                        assert late_submission['pending'] and not late_submission['published']
                        assert len(late_completed) == 1 and not late_completed[0]['pending']
                        assert not late_completed[0]['published'] and late_completed[0]['reason']
                        owned[:] = [item for item in owned if item is not bound]
                        bound = output = None
                        held.clear()
                    prepared = value = expected = step = None
                    settle(vt, device)
        del decoder, projection, marker
        assert Counter(case['case'] for case in result['cases']) == {
            'actual-h3-prefix': 6, 'alias-refusal-before-write': 2, 'retained-context-refusal': 2,
            'managed-minimum-before-write': 2, 'post-write-refusal-once': 2}
        assert len(result['keys']) == len(set(result['keys'])) == 6

    try:
        run()
        result['passed'] = True
    except Exception:
        result['error'] = traceback.format_exc()
    finally:
        try:
            # Failed Bound custody is never discarded to make cleanup appear clean.
            for bound in owned:
                if bound.submitted and not bound.completed:
                    bound.wait()
            owned.clear()
            bound = None
            settle(vt, device)
            if owner is not None:
                dense_linear.checked(owner.close, stream.cuda_stream)
            owner = None
            settle(vt, device)
            result['final'] = vt.stats(device)
            same_baseline(initial, result['final'])
        except Exception:
            result['passed'] = False
            result['cleanup_error'] = traceback.format_exc()
    return result


def require_native_ready(device):
    """An explicit fixture prerequisite waits; cold nonblocking native() may be None."""
    from functools import partial
    from cozy_runtime.internal import int8_linear, kernel_ready, prebuilt

    slot = f"{int8_linear.NATIVE} cuda:{device}"

    def state():
        pending = kernel_ready.pending()
        failure = kernel_ready.failed(slot)
        return {"state": pending.get(slot, "ready" if kernel_ready.served(slot) else "absent"),
                "pending": pending,
                "failure": None if failure is None else f"{type(failure).__name__}: {failure}"}

    record = {"slot": slot, "before": state()}
    print(json.dumps({"fixture.required_native": "before", **record}), flush=True)
    try:
        record["artifact"] = prebuilt.artifact()
        kernel_ready.wait(slot, partial(int8_linear._library, torch, device))
        assert int8_linear.native(torch, device) is not None
    except BaseException as error:
        record.update(after=state(), error=f"{type(error).__name__}: {error}")
        print(json.dumps({"fixture.required_native": "failed", **record}), flush=True)
        raise  # Preserve the actual artifact/verification error and traceback.
    record.update(after=state(), family=int8_linear.library(device).family)
    print(json.dumps({"fixture.required_native": "ready", **record}), flush=True)
    return record


@app.entrypoint(accelerator=True)
def prefix(payload: Probe, ctx: Context) -> Report:
    from cozy_runtime.graphs.domain import Domain

    torch.set_num_threads(1)
    request, vt, provenance = actual(payload, ctx)
    device = ctx.device.index or 0
    # Complete the real startup producer before the owned-stream baseline.
    startup = vt.stats(device)
    native_readiness = require_native_ready(device)
    admitted = torch.cuda.Event()
    admitted.record(torch.cuda.current_stream(device))
    admitted.synchronize()
    vt.trim(device)
    before = vt.stats(device)
    domain = Domain(torch, request.memory)
    result = {'passed': False}
    library_preparation = {}
    try:
        domain.initialize()
        domain.stream.wait_event(admitted)
        with torch.cuda.stream(domain.stream), domain.using():
            library_preparation['before'] = vt.stats(device)
            library_preparation['configuration'] = {
                'cublas_workspace_bytes': torch.backends.cuda.cublas_workspace_size(),
                'cublaslt_workspace_bytes': torch.backends.cuda.cublaslt_workspace_size(),
                'preferred_blas': str(torch.backends.cuda.preferred_blas_library()),
            }
            domain.prime_blas()
            settle(vt, device)
            library_preparation['after'] = vt.stats(device)
            result = exercise(payload, ctx, request, vt)
        result['streams'] = {'work': domain.stream.cuda_stream, 'control': domain.control.cuda_stream}
    except Exception:
        result['error'] = traceback.format_exc()
    finally:
        result.update(provenance=provenance, startup_before=startup, pre_domain=before,
                      native_readiness=native_readiness, library_preparation=library_preparation)
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
