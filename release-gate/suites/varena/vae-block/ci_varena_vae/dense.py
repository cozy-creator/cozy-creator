"""Real nn.Linear references for the six actual VAE projections; no guessed workspace."""
from __future__ import annotations

import hashlib
import inspect
import json
import traceback
from collections import Counter
from dataclasses import asdict

import torch
from cozy_runtime.author import Context, denoise_loop
from cozy_runtime.internal import dense_linear, prebuilt, verify
from cozy_runtime.internal.execution import phase
from diffusers.models.autoencoders.autoencoder_kl_minimax_h3 import (
    MiniMaxH3VideoTransformerBlock,
)
from torch.utils._python_dispatch import TorchDispatchMode

from . import Probe, Report, actual, app, settle

PATHS = ('attn.to_q', 'attn.to_k', 'attn.to_v', 'attn.to_out.0', 'ff.net.0.proj', 'ff.net.2')


def construction(device):
    with torch.device('meta'):
        block = MiniMaxH3VideoTransformerBlock(dim=2048, heads=32, dim_head=64)
    block.eval().requires_grad_(False)
    block.to_empty(device=device)
    linears = tuple(block.get_submodule(name) for name in PATHS)
    assert all(type(m) is torch.nn.Linear and m.bias is not None for m in linears)
    assert [(m.out_features, m.in_features) for m in linears] == [(2048, 2048)] * 4 + [
        (16384, 2048), (2048, 8192)]
    return block, linears


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


def arguments(plan, x, weight, output, scratch, stream):
    return (x.data_ptr(), x.numel() * x.element_size(),
            weight.data_ptr(), weight.numel() * weight.element_size(),
            output.data_ptr(), output.numel() * output.element_size(),
            0 if scratch is None else scratch.data_ptr(),
            0 if scratch is None else scratch.numel(), stream)


@app.entrypoint(accelerator=True)
def dense(payload: Probe, ctx: Context) -> Report:
    from cozy_runtime._kernels import _dense

    request, vt, provenance = actual(payload, ctx)
    device, on = ctx.device.index or 0, request.memory.device
    stream = int(torch.cuda.current_stream(device).cuda_stream)
    torch.set_num_threads(1)
    assert _dense.API == 1
    # Real stock-Lt initialization precedes the baseline. It is not proof of the
    # prepared provider's allocation closure: both preparation snapshots are retained.
    before_stock = vt.stats(device)
    with torch.no_grad(), torch.autocast('cuda', dtype=torch.float16):
        prime = torch.nn.Linear(16, 16, device=on, dtype=torch.float32).eval().requires_grad_(False)
        prime(torch.zeros(2, 16, device=on, dtype=torch.float16))
    del prime
    settle(vt, device)
    initial = vt.stats(device)
    result = {'provenance': provenance, 'stock_preparation_before': before_stock,
              'initial': initial, 'cases': [], 'keys': [],
              'scope': 'prepared dense provider only; not yet a complete fused VAE consumer',
              'library_internal_allocation_closure': 'unproven; separate driver evidence required',
              'block_source_sha256': hashlib.sha256(inspect.getsource(
                  MiniMaxH3VideoTransformerBlock).encode()).hexdigest()}
    owner = bias_buffer = None

    def run():
        nonlocal owner, bias_buffer
        owner = dense_linear.checked(_dense.Context, device, stream)
        bias_buffer = torch.empty(16384, dtype=torch.float16, device=on)
        marker = torch.zeros(1, dtype=torch.int64, device=on)
        block, linears = construction(on)
        generator = torch.Generator().manual_seed(payload.seed)
        with torch.no_grad(), torch.autocast('cuda', dtype=torch.float16):
            policy = dense_linear.policy(device)
            assert policy is not None
            result['policy'] = asdict(policy)
            for dtype in (torch.float32, torch.float16):
                for name, module in zip(PATHS, linears, strict=True):
                    module.to(dtype=dtype)
                    module.weight.copy_(torch.randn(module.weight.shape, generator=generator) * 0.02)
                    module.bias.copy_(torch.randn(module.bias.shape, generator=generator) * 0.01)
                    for tiles in (1, 3):
                        count, width, channels = tiles * 1797, module.in_features, module.out_features
                        x = (torch.randn(count, width, generator=generator) * 0.1).to(on, torch.float16)
                        reference = module(x).cpu()
                        assert reference.dtype == torch.float16
                        plan = dense_linear.checked(owner.query, count, width, channels,
                            bias_buffer.data_ptr(), bias_buffer.numel() * 2,
                            policy.half_accumulation, policy.reduction, policy.sm_target,
                            policy.workspace_limit, stream)
                        info = plan.info()
                        workspace = info['workspace_bytes']
                        assert workspace == max(info['heuristic_workspace_bytes'], info['checked_workspace_bytes'])
                        assert workspace <= policy.workspace_limit and info['alignment'] == 256
                        allocations = [count * channels * 2]
                        cast_bytes = module.weight.numel() * 2 if dtype is torch.float32 else 0
                        if cast_bytes:
                            allocations.append(cast_bytes)
                        if workspace:
                            allocations.append(workspace)
                        semantic = json.dumps((name, str(dtype), count, width, channels,
                                               asdict(policy), info['algorithm'], allocations), sort_keys=True)
                        key = 'fixture.prepared_dense/1:' + hashlib.sha256(semantic.encode()).hexdigest()
                        request.memory.remember_working_phase(key, (request,))
                        result['keys'].append(key)
                        held = {}

                        def step(i, marker, values, *, key=key, allocations=allocations,
                                 count=count, channels=channels, module=module, cast_bytes=cast_bytes,
                                 workspace=workspace, plan=plan, held=held, refuse_after=False):
                            del i, marker
                            assert dense_linear.policy(device) == policy
                            selected = vt.split_select(device, key, [('whole', allocations)],
                                                       preferred='whole', stream=stream)
                            token = phase.begin_split(device, key, 'whole', selected, stream=stream)
                            submitted = False
                            output = cast_weight = scratch = None
                            census = Census(vt, device)
                            try:
                                with census:
                                    output = torch.empty(count, channels, dtype=torch.float16, device=on)
                                    cast_weight = (torch.empty(module.weight.shape, dtype=torch.float16, device=on)
                                                   if cast_bytes else module.weight)
                                    scratch = torch.empty(workspace, dtype=torch.uint8, device=on) if workspace else None
                                    args = arguments(plan, values, cast_weight, output, scratch, stream)
                                    # All actual bindings validate before the first bias/weight/output write.
                                    dense_linear.checked(plan.validate, *args)
                                    if cast_bytes:
                                        cast_weight.copy_(module.weight)
                                    bias_buffer[:channels].copy_(module.bias)
                                    before = vt.stats(device)
                                    held['execute_calls'] = held.get('execute_calls', 0) + 1
                                    dense_linear.checked(plan.execute, *args)
                                    after = vt.stats(device)
                                    held.update(execute_before=before, execute_after=after)
                                    if refuse_after:
                                        # Instrumented failure after the actual GEMM, not a product
                                        # allocation. Request more than the whole hard managed cap.
                                        held['output'] = output
                                        cap = (after['mapped_bytes']
                                               + request.memory.native.stats()['resident_bytes'])
                                        held['injected_cap'] = cap
                                        vt.set_limit(device, cap)
                                        try:
                                            unexpected = torch.empty(cap + 2 * 1024 * 1024,
                                                                     dtype=torch.uint8, device=on)
                                            del unexpected
                                            raise AssertionError('real post-write allocation unexpectedly fit')
                                        finally:
                                            vt.set_limit(device, None)
                                submitted = True
                                held.update(output=output, selection=selected, census=census.calls)
                            except BaseException as operation_failure:
                                operation_failure.varena_no_retry = True
                                raise
                            finally:
                                held['submission'] = vt.split_end(device, token, submitted=submitted)
                                held['census'] = census.calls
                                cast_weight = scratch = output = None

                        settle(vt, device)
                        denoise_loop(step, range(1), state=(marker,), inputs=(x,), key=('prepared-dense', key))
                        output = held.pop('output')
                        observed = output.cpu()
                        error = verify.error(torch, observed, reference)
                        sample = request.working_phases[-1]
                        child = next(s for s in sample['split_observations'] if s['key'] == key)
                        case = {'case': 'actual-linear', 'module': name, 'source_dtype': str(dtype),
                                'tiles': tiles, 'query': info, 'allocations': allocations,
                                'relative_l2': error, 'bit_exact': torch.equal(observed, reference),
                                'phase': sample, **held}
                        result['cases'].append(case)
                        assert child['published'] and error <= prebuilt.BUDGET
                        assert held['execute_calls'] == 1
                        assert held['execute_before']['allocations'] == held['execute_after']['allocations']
                        assert Counter(c['live_delta'] for c in held['census'] if c['allocations']) == Counter(allocations)
                        assert sum(c['allocations'] for c in held['census']) == len(allocations)
                        assert all(c['allocations'] == 0 for c in held['census'] if 'copy_' in c['op'])
                        if name == 'ff.net.0.proj' and tiles == 1:
                            # Actual caller buffers, one deliberately wrong capacity. Validation
                            # must reject before any numerical call or mutation of the known output.
                            validation_weight = module.weight.to(torch.float16)
                            validation_scratch = (torch.empty(workspace, dtype=torch.uint8, device=on)
                                                  if workspace else None)
                            invalid = list(arguments(plan, x, validation_weight, output,
                                                     validation_scratch, stream))
                            invalid[5] -= 1
                            before = vt.stats(device)
                            refusal = None
                            try:
                                dense_linear.checked(plan.validate, *invalid)
                            except ValueError as invalid_binding:
                                refusal = str(invalid_binding)
                            after = vt.stats(device)
                            unchanged = torch.equal(output.cpu(), reference)
                            result['cases'].append({'case': 'binding-refusal', 'dtype': str(dtype),
                                'error': refusal, 'output_unchanged': unchanged,
                                'before': before, 'after': after})
                            assert refusal and unchanged and before['allocations'] == after['allocations']
                            validation_weight = validation_scratch = invalid = None
                            if dtype is torch.float32:
                                refusal = None
                                try:
                                    dense_linear.checked(owner.close, stream)
                                except ValueError as live_plan:
                                    refusal = str(live_plan)
                                result['cases'].append({'case': 'retained-context-refusal', 'error': refusal})
                                assert refusal and 'retained plans' in refusal
                        output = observed = None
                        held.clear()
                        settle(vt, device)
                        if name == 'ff.net.0.proj' and tiles == 1:
                            successful = vt.split_select(device, key, [('whole', allocations)],
                                                         preferred='whole', stream=stream)
                            assert successful['basis'] == 'measured' and successful['required_bytes'] > 0
                            refused = {}

                            def minimum(i, marker, values, *, key=key, allocations=allocations, refused=refused):
                                del i, marker, values
                                settle(vt, device)
                                probe = vt.split_select(device, key, [('whole', allocations)],
                                                        preferred='whole', stream=stream)
                                assert probe['required_bytes'] > 0
                                before = vt.stats(device)
                                cap = (before['mapped_bytes'] + request.memory.native.stats()['resident_bytes']
                                       + probe['required_bytes'] - 1)
                                vt.set_limit(device, cap)
                                try:
                                    vt.split_select(device, key, [('whole', allocations)],
                                                    preferred='whole', stream=stream)
                                except torch.OutOfMemoryError as minimum_refusal:
                                    refused['error'] = str(minimum_refusal)
                                finally:
                                    vt.set_limit(device, None)
                                refused.update(before=before, after=vt.stats(device), cap=cap, probe=probe)

                            denoise_loop(minimum, range(1), state=(marker,), inputs=(x,), key=('dense-min', key))
                            next_sample = vt.split_select(device, key, [('whole', allocations)],
                                                         preferred='whole', stream=stream)
                            result['cases'].append({'case': 'managed-minimum', 'dtype': str(dtype),
                                'prior': successful, 'after_sample': next_sample, **refused})
                            assert refused.get('error')
                            assert refused['before']['allocations'] == refused['after']['allocations']
                            assert next_sample['basis'] == 'measured'
                            assert next_sample['measured_backing_peak_bytes'] == successful['measured_backing_peak_bytes']
                            held.clear()

                            def late(i, marker, values, *, step=step):
                                return step(i, marker, values, refuse_after=True)

                            failure = None
                            try:
                                denoise_loop(late, range(1), state=(marker,), inputs=(x,), key=('dense-late', key))
                            except torch.OutOfMemoryError as allocation_refusal:
                                failure = str(allocation_refusal)
                                assert getattr(allocation_refusal, 'varena_no_retry', False)
                            assert failure and held.get('execute_calls') == 1, held
                            completed = held.pop('output')
                            observed = completed.cpu()
                            error = verify.error(torch, observed, reference)
                            late_sample = vt.split_select(device, key, [('whole', allocations)],
                                                         preferred='whole', stream=stream)
                            result['cases'].append({'case': 'post-write-refusal', 'dtype': str(dtype),
                                'error': failure, 'relative_l2': error,
                                'phase': request.working_phases[-1],
                                'prior_sample': successful, 'after_sample': late_sample,
                                'injection': 'real allocation larger than the hard managed cap after the actual dense launch',
                                **held})
                            assert error <= prebuilt.BUDGET
                            assert not held['submission']['published']
                            assert late_sample['basis'] == 'measured'
                            assert late_sample['measured_backing_peak_bytes'] == successful['measured_backing_peak_bytes']
                            completed = observed = late = None
                            held.clear()
                        plan = step = minimum = x = reference = None
                        settle(vt, device)
                # Modules retain only the latest real source dtype, exactly as the next reference uses.
            # The final context may not close while a real plan still retains it; normal cleanup
            # below drops all locals and drains their actual stream before explicit owner close.
        del marker, block, linears
        assert Counter(case['case'] for case in result['cases']) == {
            'actual-linear': 24, 'managed-minimum': 2, 'binding-refusal': 2,
            'post-write-refusal': 2, 'retained-context-refusal': 1}
        assert len(result['keys']) == len(set(result['keys'])) == 24

    try:
        run()
        result['passed'] = True
    except Exception:  # noqa: BLE001 -- preserve the complete failed mechanism report.
        result['passed'] = False
        result['error'] = traceback.format_exc()
    finally:
        try:
            settle(vt, device)
            if owner is not None:
                dense_linear.checked(owner.close, stream)
            owner = bias_buffer = None
            settle(vt, device)
            result['final'] = vt.stats(device)
            for name in ('allocated_bytes', 'allocated_capacity', 'mapped_bytes', 'pending_bytes',
                         'pools', 'pools_bytes', 'event_handles'):
                assert result['final'][name] == initial[name], (name, initial[name], result['final'][name])
        except Exception:  # noqa: BLE001 -- retain the first error as well as cleanup evidence.
            result['passed'] = False
            result['cleanup_error'] = traceback.format_exc()
    return Report(result['passed'], json.dumps(result, sort_keys=True))
