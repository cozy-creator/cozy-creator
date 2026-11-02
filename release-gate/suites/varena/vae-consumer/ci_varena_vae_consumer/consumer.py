"""Real fused block execution, independent buffer census and allocator refusals."""
from __future__ import annotations

import json
import math
import tempfile
import threading
import traceback
from collections import Counter
from contextlib import contextmanager
from pathlib import Path

import torch
from cozy_runtime.author import Context, denoise_loop
from cozy_runtime.author.kernels import read
from cozy_runtime.graphs.domain import Domain
from cozy_runtime.internal import dense_linear, prebuilt, triton_kernels, verify
from torch.utils._python_dispatch import TorchDispatchMode

from . import Probe, Report, actual, app, readiness, settle
from .model import Loaded, selected
from .model import rotary as make_rotary

BATCH, TOKENS, WIDTH = 3, 1797, 2048


class Census(TorchDispatchMode):
    def __init__(self, vt, device):
        super().__init__()
        self.vt, self.device, self.calls, self.output = vt, device, [], None
        self.inference, self.grad = torch.is_inference_mode_enabled(), torch.is_grad_enabled()

    def __torch_dispatch__(self, func, types, args=(), kwargs=None):
        active = self.vt.split_context(self.device)['reason'] == 'leaf_active'
        before = self.vt.stats(self.device)
        with torch.inference_mode(self.inference), torch.set_grad_enabled(self.grad):
            result = func(*args, **(kwargs or {}))
        after = self.vt.stats(self.device)
        row = {'op': str(func), 'leaf': active,
               'allocations': after['allocations'] - before['allocations'],
               'live_delta': after['allocated_bytes'] - before['allocated_bytes']}
        self.calls.append(row)
        if active and row['allocations'] and isinstance(result, torch.Tensor):
            row['shape'], row['dtype'] = list(result.shape), str(result.dtype)
            if result.dtype is torch.float32 and tuple(result.shape) == (BATCH, TOKENS, WIDTH):
                assert self.output is None
                self.output = result
        return result


def expected_allocations(record, storage):
    batch = record['batch_chunk']
    rows = {batch * TOKENS, (BATCH % batch) * TOKENS} - {0}
    queried = [item for item in record['algorithms'] if item[0][0] in rows]
    assert {tuple(item[0][1:]) for item in queried} == {(2048, 2048), (2048, 16384), (8192, 2048)}
    scratch = max(item[2] for item in queried)
    values = [BATCH * TOKENS * WIDTH * 4, batch * TOKENS * 16384 * 2,
              batch * TOKENS * 8192 * 2]
    if storage is torch.float32:
        values.append(16384 * 2048 * 2)
    if scratch:
        values.append(scratch)
    values += [batch * 32 * TOKENS * 64] * 2
    values += [batch * 32 * 64 * (math.ceil(TOKENS / 64) * 64),
               batch * 32 * math.ceil(TOKENS / 128) * 32 * 4,
               batch * 32 * math.ceil(TOKENS / 64) * 4 * 4,
               batch * 32 * 64 * 4, batch * 32 * 4]
    return values


@contextmanager
def observe(request, vt, device, held, mode, census):
    """Forward real operations; only the named refusal cells add a real cap/allocation."""
    from cozy_runtime.fusion import _kernels

    thread = threading.get_ident()
    select, checked = vt.split_select, dense_linear.checked
    launch = triton_kernels._Launcher.__call__
    held.update(queries=[], dense_calls=[], launches=[], probe_keys=[])
    stream = int(torch.cuda.current_stream(device).cuda_stream)

    def choosing(device, key, choices, *, preferred, **kwargs):
        if not key.startswith('runtime.split.layer_scale_block/1:'):
            return select(device, key, choices, preferred=preferred, **kwargs)
        held.update(key=key, choices=choices)
        if mode in ('cut', 'minimum', 'late'):
            at = 1 if mode == 'minimum' else 2
            choice, vector = next(row for row in choices if row[0].startswith(f'batch:{at}/'))
            probe_key = f'fixture.vae.capacity/{key}/{mode}'
            request.memory.remember_working_phase(probe_key, (request,))
            held['probe_keys'].append(probe_key)
            probe = select(device, probe_key, [(choice, vector)], preferred=choice, **kwargs)
            assert probe['required_bytes'] > 0, probe
            stats = vt.stats(device)
            cap = stats['mapped_bytes'] + request.memory.native.stats()['resident_bytes'] + probe['required_bytes']
            if mode == 'minimum':
                cap -= 1
            held.update(probe=probe, cap=cap, before_selection=stats)
            vt.set_limit(device, cap)
        result = select(device, key, choices, preferred=preferred, **kwargs)
        held['selection'] = dict(result)
        return result

    def calling(call, *args, **kwargs):
        ours = threading.get_ident() == thread
        name = getattr(call, '__name__', '')
        if ours and name in ('query', 'validate', 'execute'):
            assert args[-1] == stream
        before = vt.stats(device) if ours and name == 'execute' else None
        result = checked(call, *args, **kwargs)
        if ours and name == 'query':
            held['queries'].append(result.info())
        if ours and name == 'execute':
            after = vt.stats(device)
            held['dense_calls'].append({'stream': args[-1], 'allocations': after['allocations'] - before['allocations']})
            assert args[-1] == stream
        return result

    def launching(self, kernel, grid, *args):
        ours = threading.get_ident() == thread
        active = vt.split_context(device)['reason'] == 'leaf_active' if ours else False
        if ours:
            assert int(torch.cuda.current_stream(device).cuda_stream) == stream
        result = launch(self, kernel, grid, *args)
        if ours:
            row = {'kernel': kernel.__name__, 'grid': list(grid), 'leaf': active, 'stream': stream}
            held['launches'].append(row)
            if kernel is _kernels.norm_modulate_kernel:
                constants = dict(zip(_kernels.ROW_CONSTEXPRS, args[-len(_kernels.ROW_CONSTEXPRS):], strict=True))
                row['constants'] = constants
                if mode == 'late' and constants['NORM'] == 0 and active:
                    assert census.output is not None
                    held['completed_batch'] = 2
                    stats = vt.stats(device)
                    cap = stats['mapped_bytes'] + request.memory.native.stats()['resident_bytes']
                    held['injected_cap'] = cap
                    vt.set_limit(device, cap)
                    unexpected = torch.empty(cap + 2 * 1024 * 1024, device=request.memory.device, dtype=torch.uint8)
                    del unexpected
                    raise AssertionError('real post-write allocation unexpectedly fit')
        return result

    vt.split_select, dense_linear.checked = choosing, calling
    triton_kernels._Launcher.__call__ = launching
    try:
        yield
    finally:
        vt.split_select, dense_linear.checked = select, checked
        triton_kernels._Launcher.__call__ = launch
        vt.set_limit(device, None)


def one(request, vt, loaded, x, rotary, marker, storage, mode, reference, records):
    device = x.device.index or 0
    held = {}
    before_moved = request.moved

    def step(i, marker, values, cosine, sine):
        del i, marker
        census = Census(vt, device)
        held['census_owner'] = census
        with census, observe(request, vt, device, held, mode, census):
            held['output'] = loaded.run(values, (cosine, sine))

    failure = None
    try:
        denoise_loop(step, range(1), state=(marker,), inputs=(x, *rotary),
                     key=('actual-vae-block', str(storage), str(x.dtype), mode))
    except torch.OutOfMemoryError as error:
        failure = str(error)
        held['no_retry'] = bool(getattr(error, 'varena_no_retry', False))
    except BaseException:
        census = held.pop('census_owner', None)
        held.pop('output', None)
        held.update(case=mode, matrix_dtype=str(storage), residual_dtype=str(x.dtype),
                    census=[] if census is None else census.calls, error=traceback.format_exc())
        records.append(held)
        raise
    census = held.pop('census_owner')
    output = held.pop('output', None)
    held['source_bytes_read'] = request.moved - before_moved
    held['census'] = census.calls
    held['phase'] = request.working_phases[-1]
    held['case'] = mode
    held['matrix_dtype'], held['residual_dtype'] = str(storage), str(x.dtype)
    held['error'] = failure
    records.append(held)  # preserve the full mechanism report before every verdict
    if failure:
        assert held['no_retry']
    key = held['key']
    if mode == 'minimum':
        assert failure and output is None and census.output is None
        assert not held['dense_calls'] and not held['launches']
        assert not any(row['leaf'] and row['allocations'] for row in census.calls)
        return held, None
    record = request.working_splits[key]
    held['record'] = dict(record)
    actual_queries = {(q['rows'], q['inputs'], q['outputs']):
                      (q['algorithm'], q['workspace_bytes'], q['library_version'], q['header_version'])
                      for q in held['queries']}
    declared_queries = {tuple(row[0]): tuple(row[1:]) for row in record['algorithms']}
    assert actual_queries == declared_queries and len(actual_queries) == 9
    target_batch = 3 if mode == 'whole' else 2
    assert record['batch_chunk'] == target_batch and record['invocation_count'] == 1
    allocations = expected_allocations(record, storage)
    assert list(dict(record['choices'])[held['selection']['choice_id']]) == allocations
    assert Counter(row['live_delta'] for row in census.calls if row['leaf'] and row['allocations']) == Counter(allocations)
    assert all(row['allocations'] == 0 for row in held['dense_calls'])
    assert all(row['leaf'] for row in held['launches'])  # no numerical warm forward before admission
    assert not any(row['op'].startswith('aten.addmm') for row in census.calls)
    if mode == 'late':
        assert failure and output is None and census.output is not None
        output = census.output
        got = output[:2].cpu()
        want = reference[:2]
        assert len(held['dense_calls']) == 6
        assert not record['submission']['published']
    else:
        assert failure is None and output is census.output
        got, want = output.cpu(), reference
        expected_windows = math.ceil(BATCH / target_batch)
        assert len(held['dense_calls']) == 6 * expected_windows
        kinds = Counter(row['kernel'] for row in held['launches'])
        assert kinds == {'norm_modulate_kernel': 3 * expected_windows,
                         'head_norm_rope_kernel': 2 * expected_windows,
                         'swiglu_kernel': expected_windows}, kinds
        child = next(row for row in held['phase']['split_observations'] if row['key'] == key)
        assert child['published'] and not child['pending']
    error = verify.error(torch, got, want)
    held.update(relative_l2=error, bit_exact=torch.equal(got, want))
    assert error <= prebuilt.BUDGET
    census.output = None
    return held, output


@app.entrypoint(accelerator=True)
def consumer(payload: Probe, ctx: Context) -> Report:
    request, vt, provenance = actual(payload, ctx)
    device, on = ctx.device.index or 0, request.memory.device
    torch.set_num_threads(1)
    startup = readiness(vt, device)
    parent = torch.cuda.current_stream(device)
    admitted = torch.cuda.Event()
    admitted.record(parent)
    admitted.synchronize()
    vt.trim(device)
    before = vt.stats(device)
    domain = Domain(torch, request.memory)
    result = {'passed': False, 'provenance': provenance, 'cases': [], 'keys': [],
              'pre_domain': before, 'startup_readiness': startup, 'scope': 'actual standalone fused VAE block; outer decoder remains unmigrated',
              'library_internal_allocation_closure': 'unproven without separate driver evidence'}
    loaded = last_output = last_host = None
    try:
        domain.initialize()
        domain.stream.wait_event(admitted)
        result['streams'] = {'work': int(domain.stream.cuda_stream), 'control': int(domain.control.cuda_stream)}
        assert all(result['streams'].values())
        with torch.cuda.stream(domain.stream), domain.using(), torch.no_grad(), torch.autocast('cuda', dtype=torch.float16):
            domain.prime_blas()
            backend, ready = selected(on)
            result['kitchen_artifact'] = ready.artifact
            # Kernel/stock library preparation precedes the retained lifecycle baseline.
            from minimax_h3.vae_tiles import VAE_BLOCK
            triton_kernels.load(VAE_BLOCK.kernels, device=on)
            settle(vt, device)
            initial = vt.stats(device)
            result['initial'] = initial
            initial_weights = request.memory.native.stats()
            result['initial_weights'] = initial_weights
            generator = torch.Generator().manual_seed(payload.seed)
            rotary = make_rotary(on, TOKENS)
            marker = torch.zeros(1, dtype=torch.int64, device=on)
            for storage in (torch.float32, torch.float16):
                with tempfile.TemporaryDirectory(prefix='varena-vae-consumer-') as folder:
                    loaded = Loaded(request, Path(folder), storage, backend)
                    for residual in (torch.float16, torch.float32):
                        x = (torch.randn((BATCH, TOKENS, WIDTH), generator=generator) * .1).to(on, residual)
                        references = {batch: loaded.expected(x, rotary, batch) for batch in (3, 2)}
                        for mode in ('whole', 'cut'):
                            settle(vt, device)
                            case, output = one(request, vt, loaded, x, rotary, marker, storage, mode,
                                               references[3 if mode == 'whole' else 2], result['cases'])
                            result['keys'].extend([case['key'], *case['probe_keys']])
                            assert isinstance(output, torch.Tensor)
                            if mode == 'cut':
                                escaped, escaped_cpu = output, output.cpu()
                                last_output, last_host = escaped, escaped_cpu
                            output = None
                        prior = next(row for row in case['phase']['split_observations']
                                     if row['key'] == case['key'])
                        for mode in ('minimum', 'late'):
                            settle(vt, device)
                            case, diagnostic = one(request, vt, loaded, x, rotary, marker, storage, mode,
                                                   references[2], result['cases'])
                            result['keys'].extend([case['key'], *case['probe_keys']])
                            assert case['key'] == prior['key']
                            assert torch.equal(escaped.cpu(), escaped_cpu)
                            if mode == 'late':
                                assert diagnostic is not None
                                after = next(row for row in case['phase']['split_observations']
                                             if row['key'] == prior['key'])
                                assert after['choice_id'] == prior['choice_id']
                                assert after['successful_passes'] == prior['successful_passes']
                                assert case['selection']['basis'] == 'measured'
                                assert case['selection']['measured_backing_peak_bytes'] == prior['backing_peak_bytes']
                                case['prior_success_preserved'] = True
                            else:
                                assert diagnostic is None
                            diagnostic = None
                        # A real escaped read alias protects the source region from explicit reclaim.
                        alias = read(loaded.block.norm1.weight)
                        alias_cpu = alias.cpu().clone()
                        pinned_before = request.memory.native.stats()
                        assert pinned_before['resident_bytes'] > 0 and pinned_before['pinned_bytes'] > 0
                        reclaimed = request.memory.native.reclaim(pinned_before['resident_bytes'])
                        pinned_after = request.memory.native.stats()
                        assert torch.equal(alias.cpu(), alias_cpu)
                        assert pinned_after['pinned_bytes'] == pinned_before['pinned_bytes']
                        case['source_alias_preserved'] = True
                        case['alias_reclaim'] = {'requested': pinned_before['resident_bytes'],
                            'returned': reclaimed, 'before': pinned_before, 'after': pinned_after}
                        alias = alias_cpu = x = escaped = escaped_cpu = references = None
                        settle(vt, device)
                    loaded.close()
                    loaded = None
                    request.memory.native.reclaim(request.memory.native.stats()['reclaimable_bytes'])
                    assert last_output is not None and torch.equal(last_output.cpu(), last_host)
                    case['escaped_output_survived_source_close'] = True
                    last_output = last_host = None
            marker = rotary = None
            settle(vt, device)
            result['final'] = vt.stats(device)
            result['final_weights'] = request.memory.native.stats()
            assert not request.memory.models
            for field in ('resident_bytes', 'reserved_bytes', 'pinned_bytes'):
                assert result['final_weights'][field] == initial_weights[field]
            for field in ('allocated_bytes', 'allocated_capacity', 'mapped_bytes', 'pending_bytes',
                          'pools', 'pools_bytes', 'event_handles'):
                assert result['final'][field] == initial[field], (field, initial, result['final'])
            assert Counter(row['case'] for row in result['cases']) == {
                'whole': 4, 'cut': 4, 'minimum': 4, 'late': 4}
            result['passed'] = True
    except Exception:  # noqa: BLE001 -- keep complete partial evidence.
        result['error'] = traceback.format_exc()
        result['passed'] = False
    finally:
        try:
            if loaded is not None:
                loaded.close()
            domain.finish()
            vt.trim(device)
            result['post_domain'] = vt.stats(device)
            for field in ('allocated_bytes', 'allocated_capacity', 'mapped_bytes', 'pending_bytes',
                          'pools', 'pools_bytes', 'event_handles'):
                assert result['post_domain'][field] == before[field], (field, before, result['post_domain'])
            result['owned_streams_retired'] = True
        except Exception:  # noqa: BLE001
            result['cleanup_error'] = traceback.format_exc()
            result['passed'] = False
    result['keys'] = sorted(set(result['keys']))
    return Report(result['passed'], json.dumps(result, sort_keys=True))
