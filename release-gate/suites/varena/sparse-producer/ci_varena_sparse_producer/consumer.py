"""Actual registered Runtime Processor, package sparse Heads, and Loop-carried statistics."""
from __future__ import annotations

import gc
import json
from collections import Counter
from contextlib import nullcontext
from contextvars import ContextVar
from dataclasses import replace

import torch
from cozy_runtime.author import (
    AttentionLayout,
    Context,
    ProducerSite,
    Telemetry,
    attention_scope,
    denoise_loop,
    register_attention,
)
from minimax_h3 import glue, producer, producer_kernels
from torch.utils._python_dispatch import TorchDispatchMode

from . import Probe, Report, actual, app, settle

WIDTH = 5376
_CASE: ContextVar[dict | None] = ContextVar('sparse_consumer_fixture', default=None)
_ORIGINAL = producer.PRODUCERS[producer.INT4QK]


def reference(call, hidden, rotary, *, commit=False, complete=False):
    """The actual ordinary package callable, with its real Kitchen implementation."""
    tokens, heads = call.layout.live_tokens, call.module.heads
    freqs = producer.rope_presentation(rotary, tokens, call.state)
    means = producer._Means(call, (call.component, call.path), producer.INT4QK,
                            (tokens, heads, call.layout.protected_prefix))
    operation = producer._Heads.of(call, means, hidden, freqs)
    assert type(operation) is producer._Heads
    output = operation(operation)
    assert means.mean is not None
    if commit:
        means.close()
        return output
    if complete:
        output = call.module.to_out[1](call.module.to_out[0](output.flatten(2, 3)))
    return output.cpu(), means.mean.cpu()


def menu(native, kitchen, device, tokens, heads, chunk, rolling):
    """Independent native queries and concrete storage, never a replay-based byte probe."""
    windows = tuple(dict.fromkeys((chunk, heads % chunk or chunk)))
    rows = tuple(dict.fromkeys((min(4096, tokens), tokens % 4096 or min(4096, tokens))))
    queries, plans = [], {}
    for h in windows:
        plans[h] = kitchen.sol_attn_plan(1, tokens, h, token_aug=0)
        for m in rows:
            for packed in ((False, True) if rolling else (True,)):
                pitch = h * 128 * (3 if packed else 1)
                found = native.int8_plan(device, False, m, h * 128, WIDTH, pitch)
                assert found['status'] == 0
                if packed:
                    assert found['kind'] == 0, 'packed output cannot take contiguous-only GEMV'
                queries.append({'rows': m, 'heads': h, 'packed': packed, 'pitch': pitch, **found})
    qrows = max((q['rows'] for q in queries if q['kind'] == 0), default=0)
    gemm = max(q['workspace_bytes'] for q in queries)
    storage = [2 * tokens * heads * 128, 4 * heads * 128,
               2 * min(4096, tokens) * 3 * chunk * 128]
    if chunk < heads:
        storage.append(2 * tokens * chunk * 128)
    if qrows:
        storage += [qrows * WIDTH, qrows * 4]
    if gemm:
        storage.append(gemm)
    storage += [max(p['total'] for p in plans.values()), 4 * chunk * 128, 4 * chunk * 128]
    if not rolling:
        storage += [4 * chunk * 128, 4]
    return storage, {'projections': queries, 'sol': plans, 'allocations': storage}


class Census(TorchDispatchMode):
    def __init__(self, vt, device, slot, shape, after_write):
        super().__init__()
        self.vt, self.device, self.slot, self.shape = vt, device, slot, shape
        self.after_write = after_write
        self.rows, self.commits, self.joins = [], 0, 0
        self.output_pointer, self.output, self.boundary = None, None, None

    def __torch_dispatch__(self, func, types, args=(), kwargs=None):
        active = self.vt.split_context(self.device)['reason'] == 'leaf_active'
        before = self.vt.stats(self.device) if active else None
        result = func(*args, **(kwargs or {}))
        if not active:
            return result
        after = self.vt.stats(self.device)
        allocated = after['allocations'] - before['allocations']
        storage = []
        if allocated:
            assert allocated == 1 and isinstance(result, torch.Tensor) and result.is_cuda
            storage = [{'requested_bytes': result.numel() * result.element_size(),
                        'actual_class_bytes': after['allocated_capacity'] - before['allocated_capacity'],
                        'shape': list(result.shape), 'dtype': str(result.dtype)}]
            if tuple(result.shape) == self.shape:
                self.output_pointer = result.untyped_storage().data_ptr()
                if self.after_write:
                    self.output = result
        role = ''
        if str(func) == 'aten.copy_.default':
            address = args[0].untyped_storage().data_ptr()
            if address == self.slot.untyped_storage().data_ptr():
                role = 'carried-mean-commit'
                self.commits += 1
            elif address == self.output_pointer:
                role = 'head-output-join'
                self.joins += 1
        self.rows.append({'op': str(func), 'role': role, 'allocations': allocated, 'storage': storage})
        if self.after_write and role == 'head-output-join' and self.joins == 1:
            torch.cuda.current_stream(self.device).synchronize()
            assert self.output is not None and self.commits == 0
            self.boundary = {'stage': 'first-head-window-complete'}
            start = self.vt.stats(self.device)
            first_head = torch.empty((1, self.output.shape[1], 1, 128),
                                     dtype=self.output.dtype, device='cpu')
            for row in range(self.output.shape[1]):
                value = self.output[0, row, 0]
                assert value.is_contiguous()
                first_head[0, row, 0].copy_(value)
            self.boundary.update(first_head=first_head, mean=self.slot.cpu())
            end = self.vt.stats(self.device)
            assert end['allocations'] == start['allocations']
            self.boundary['readback_device_allocations'] = 0
            limit = end['mapped_bytes']
            self.vt.set_limit(self.device, limit)
            self.boundary['stage'] = 'real-allocation-refusal-requested'
            torch.empty(limit + (2 << 20), dtype=torch.uint8, device=self.slot.device)
            raise AssertionError('the deliberate managed allocation unexpectedly succeeded')
        return result


def observed_produce(call, hidden, rotary=None, attention_mask=None):
    case = _CASE.get()
    if case is None:
        return _ORIGINAL.produce(call, hidden, rotary, attention_mask)
    assert call.module is case['attn'] and call.state is _OBSERVED.request_state()
    assert attention_mask is None
    if case['seed_step']:
        # A real complete ordinary head operation seeds the actual carried slot.
        return reference(call, hidden, rotary, commit=True)
    slot, vt, device = case['slot'], case['vt'], case['device']
    case['prior'] = slot.cpu()
    case['carried_before'] = id(call.module) in call.state.carried
    case['reference'], case['reference_mean'] = reference(
        call, hidden, rotary, complete=case['complete'])
    settle(vt, device)
    before = vt.stats(device)
    case['before'] = before
    census = Census(vt, device, slot, (1, call.layout.live_tokens, call.module.heads, 128),
                    case['chunk'] == 'after-write')
    case['census'] = census
    key = f"fixture.sparse-capacity/{case['name']}"
    try:
        if case['chunk'] == 'planned':
            from tensorfs import varena
            case['weights_before'] = call_memory = case['request'].memory.native.stats()
            assert call_memory['pinned_bytes'] == 0
            region = case['request'].memory.native.reserve(2 << 20)
            use = region.prepare()
            tensor = use.tensor
            use.consume_on(int(torch.cuda.current_stream(device).cuda_stream))
            assert use.needs_fill
            tensor.fill_(53)
            use.commit()
            del tensor, use
            case['region'] = region
            case['plan'] = varena.Plan(case['request'].memory.native, [('sparse-call', [region])])
            case['plan'].activate('sparse-call')
        if not case['complete']:
            selected = vt.split_select(device, key, [('target', case['allocations'])],
                                       preferred='target', stream=int(torch.cuda.current_stream(device).cuda_stream))
            if case['chunk'] is None:
                assert selected['required_bytes'] > 0
            weights = case['request'].memory.native.stats()['resident_bytes']
            vt.set_limit(device, before['mapped_bytes'] + weights + selected['required_bytes']
                         - (1 if case['chunk'] is None else 0))
            case['capacity'] = selected
        with census if not case['complete'] else nullcontext():
            if case['complete']:
                return _ORIGINAL.produce(call, hidden, rotary, attention_mask)
            output, reuse = producer._attend(producer.INT4QK, call,
                                             (call.component, call.path), hidden, rotary)
            assert reuse is (case['mode'] == 'rolling')
            return output
    finally:
        vt.set_limit(device, None)
        vt.phase_forget(device, key)
        if 'plan' in case:
            case['plan'].deactivate()


# This real registered Processor forwards to the package implementation. The module-level
# observer and ContextVar retain no per-case tensor after its token is reset.
_OBSERVED = register_attention(replace(_ORIGINAL, produce=observed_produce))


def make_attention(on, generator, launch, stats, selected, heads):
    from cozy_runtime import lora
    from cozy_runtime.internal import producers
    from cozy_runtime.internal.encoding.formats import CONVROT_GROUP
    from cozy_runtime.internal.encoding.leaves import Int8ConvRotNativeLeaf
    from diffusers.models.transformers.transformer_minimax_h3 import MiniMaxH3Attention

    with torch.device('meta'):
        attn = MiniMaxH3Attention(WIDTH, heads, 128)
    attn.norm_q = torch.nn.RMSNorm(128, eps=1e-5, dtype=torch.bfloat16, device=on)
    attn.norm_k = torch.nn.RMSNorm(128, eps=1e-5, dtype=torch.bfloat16, device=on)
    provider = Int8ConvRotNativeLeaf()
    params = {'rotation': 'kron_regular_h4', 'rotation_size': CONVROT_GROUP}
    reads = [0]

    def armed():
        reads[0] += 1

    for parent, name, channels, columns in [
        *((attn, n, heads * 128, WIDTH) for n in ('to_q', 'to_k', 'to_v')),
        (attn.to_out, '0', WIDTH, heads * 128),
    ]:
        refusal = provider.supports((channels, columns), params)
        if refusal:
            assert parent is attn.to_out and heads == 55
            continue
        leaf = provider.leaf(torch, {
            'weight': torch.randint(-127, 128, (channels, columns), generator=generator,
                                    dtype=torch.int8).to(on),
            'scale': torch.full((channels,), 0.001, dtype=torch.float32, device=on),
        }, getattr(parent, name), torch.bfloat16)
        hook = lora.Hook(armed)
        hook._cozy_baked = True
        leaf.register_forward_hook(hook)
        setattr(parent, name, leaf)
    processor = glue.FusedMiniMaxH3AttnProcessor()
    processor._glue, processor._producer_stats = launch, stats
    processor._attention_backend, processor._parallel_config = selected, None
    attn.set_processor(processor)
    producers.install(attn, _OBSERVED, str(selected.value), 'comfy_kitchen', 'fixture', 'attention')
    return attn.eval(), reads


def equal(actual, expected):
    from cozy_runtime.internal import prebuilt, verify
    error = verify.error(torch, actual, expected)
    assert torch.isfinite(actual).all() and error <= prebuilt.BUDGET
    return {'relative_l2': error, 'budget': prebuilt.BUDGET, 'exact': torch.equal(actual, expected)}


@app.entrypoint(accelerator=True)
def consumer(payload: Probe, ctx: Context, tel: Telemetry) -> Report:
    from cozy_runtime import _kernels
    from cozy_runtime.internal import attention, int8_linear, triton_kernels
    from cozy_runtime.internal.encoding.selection import measure_device

    request, vt, provenance = actual(payload, ctx)
    device = ctx.device.index or 0
    on = torch.device('cuda', device)
    torch.set_num_threads(1)
    assert int8_linear.native(torch, device) is not None
    ready, = attention.pinned(_OBSERVED.name, measure_device(torch, device, 'cuda'))
    from comfy_kitchen.backends import cuda as kitchen
    launch = triton_kernels.load(glue.kernels(on), device=on)
    stats = triton_kernels.load(producer_kernels.STATS, device=on)
    provenance.update(launcher_key=stats.key, backend=str(ready.member.value))
    generator = torch.Generator().manual_seed(payload.seed)
    settle(vt, device)
    initial = vt.stats(device)
    initial_weights = request.memory.native.stats()
    cases = []
    state = _OBSERVED.request_state()
    assert isinstance(state, producer.RequestState)
    try:
        with torch.inference_mode():
            for tokens, heads, chunks in ((8195, 56, (56, 28, 7, 1)),
                                          (8193, 56, (28,)), (513, 55, (55, 28))):
                attn, reads = make_attention(on, generator, launch, stats, ready.member, heads)
                previous = torch.randn((1, tokens, WIDTH), generator=generator,
                                       dtype=torch.bfloat16).to(on)
                current = (torch.randn((1, tokens, WIDTH), generator=generator,
                                       dtype=torch.bfloat16) * 3).to(on)
                angles = torch.randn((tokens, 96), generator=generator).to(on)
                rotary = angles.cos(), angles.sin()
                del angles
                marker = torch.zeros(1, dtype=torch.int64, device=on)
                slot = torch.full((heads, 128), 19., dtype=torch.float32, device=on)
                for mode in ('bootstrap', 'rolling'):
                    variants = [*chunks, None]
                    if tokens == 8195:
                        variants += ['after-write', 'complete']
                    if tokens == 513:
                        variants.append('planned')
                    for chunk in variants:
                        selected = 28 if chunk in ('after-write', 'planned') else 1 if chunk is None else chunk
                        complete = chunk == 'complete'
                        allocations, query = menu(_kernels._gemm, kitchen._C, device, tokens, heads,
                                                  heads if complete else selected, mode == 'rolling')
                        slot.fill_(19.)
                        producer.present((ProducerSite(attn, 'fixture', 'attention', _OBSERVED),),
                                         rotary, tokens)
                        assert not state.carried
                        case = {'name': f'{tokens}/{heads}/{mode}/{chunk}', 'attn': attn,
                                'device': device, 'vt': vt, 'mode': mode, 'chunk': chunk,
                                'complete': complete, 'allocations': allocations, 'request': request}
                        before_selections = {key: row['selections'] for key, row in request.working_splits.items()}

                        def step(i, marker, slot, previous, current, cos, sin, *, case=case,
                                 attn=attn, tokens=tokens):
                            del marker
                            assert not slot.is_inference()
                            case['seed_step'] = case['mode'] == 'rolling' and i == 0
                            case['slot'] = slot
                            token = _CASE.set(case)
                            try:
                                with (
                                    producer.carrying(producer.Carry({id(attn): slot}, rolling=i > 0)),
                                    attention_scope(AttentionLayout(tokens, 64, step=i, dense_until_step=0)),
                                ):
                                    try:
                                        result = attn(previous if case['seed_step'] else current,
                                                      (cos, sin), None)
                                    except torch.OutOfMemoryError as error:
                                        assert case['chunk'] in (None, 'after-write')
                                        assert error.varena_no_retry
                                        case['refusal'] = {'no_retry': True, 'type': type(error).__name__}
                                    else:
                                        if not case['seed_step']:
                                            case['output'] = result.cpu()
                                        del result
                            finally:
                                _CASE.reset(token)

                        denoise_loop(step, range(2 if mode == 'rolling' else 1), state=(marker,),
                                     carried=(slot,), inputs=(previous, current, *rotary),
                                     key=('actual-sparse-producer', tokens, heads, mode))
                        phase = request.working_phases[-1]
                        census = case['census']
                        records = [row for key, row in request.working_splits.items()
                                   if row['site'] == 'sparse-projected-heads'
                                   and row['selections'] > before_selections.get(key, 0)]
                        tel.log(json.dumps({'sparse_case': case['name'], 'native': len(records),
                                            'refused': 'refusal' in case,
                                            'commits': census.commits, 'joins': census.joins,
                                            'phase': phase}, sort_keys=True))
                        row = {'tokens': tokens, 'heads': heads, 'mode': mode, 'chunk': chunk,
                               'query': query, 'phase': phase}
                        if chunk is None:
                            assert 'refusal' in case and 'output' not in case and not records
                            assert not any(r['allocations'] for r in census.rows)
                            assert torch.equal(slot.cpu(), case['prior']) and census.commits == 0
                            assert (id(attn) in state.carried) == case['carried_before']
                            assert phase['invalidated'] and not phase['published']
                            row.update(case='minimum-refusal', unchanged_prior=True, refusal=case['refusal'])
                        else:
                            assert len(records) == 1
                            record = records[0]
                            assert record['statistics'] == mode
                            if not complete:
                                assert record['head_chunk'] == selected
                                observed = [s for r in census.rows for s in r['storage']]
                                assert Counter(s['requested_bytes'] for s in observed) == Counter(allocations)
                                assert all(r['op'].startswith(('aten.empty.', 'aten.empty_like.'))
                                           for r in census.rows if r['storage'])
                                assert all(r['allocations'] == 0 for r in census.rows if r['role'])
                                row.update(allocation_census=census.rows)
                            if chunk == 'after-write':
                                assert 'refusal' in case and 'output' not in case
                                assert census.boundary is not None and census.commits == 0
                                assert census.boundary['stage'] == 'real-allocation-refusal-requested'
                                assert torch.equal(slot.cpu(), case['prior'])
                                assert torch.equal(census.boundary['mean'], case['prior'])
                                assert (id(attn) in state.carried) == case['carried_before']
                                assert phase['invalidated'] and not phase['published']
                                row.update(case='post-write-refusal', unchanged_prior=True,
                                           first_output_head=equal(census.boundary['first_head'], case['reference'][:, :, :1]),
                                           readback_device_allocations=0, refusal=case['refusal'])
                            else:
                                samples = phase['split_observations']
                                assert len(samples) == (2 if complete else 1)
                                assert all(s['published'] and not s['pending'] for s in samples)
                                if not complete:
                                    assert census.commits == 1
                                assert id(attn) in state.carried
                                if chunk == 'planned':
                                    assert record['selection']['plan_generation'] > 0
                                    assert record['selection']['preserves_phase_weights']
                                    assert record['selection']['phase_weight_bytes'] == case['region'].allocated_bytes
                                    use = case['region'].prepare()
                                    tensor = use.tensor
                                    use.consume_on(int(torch.cuda.current_stream(device).cuda_stream))
                                    assert not use.needs_fill and int(tensor[0]) == int(tensor[-1]) == 53
                                    del tensor, use
                                row.update(case='complete-processor' if complete else 'whole-or-head-window',
                                           output=equal(case['output'], case['reference']),
                                           mean=equal(slot.cpu(), case['reference_mean']),
                                           native_selection=json.loads(json.dumps(record)))
                        assert reads == [0]
                        cases.append(row)
                        census.output = census.boundary = None
                        weights_before = case.get('weights_before')
                        case.clear()
                        del census, case, step
                        if weights_before is not None:
                            gc.collect()
                            torch.cuda.synchronize(device)
                            resident = request.memory.native.stats()['resident_bytes']
                            reclaimed = request.memory.native.reclaim(
                                max(0, resident - weights_before['resident_bytes']))
                            weights_after = request.memory.native.stats()
                            for field in ('resident_bytes', 'reserved_bytes', 'pinned_bytes'):
                                assert weights_after[field] == weights_before[field]
                            row['region_cleanup'] = {'before': weights_before, 'after': weights_after,
                                                     'explicitly_reclaimed_bytes': reclaimed}
                del attn, previous, current, rotary, marker, slot
                state.close()
    finally:
        vt.set_limit(device, None)
        state.close()
    settle(vt, device)
    final = vt.stats(device)
    final_weights = request.memory.native.stats()
    for field in ('allocated_bytes', 'allocated_capacity', 'mapped_bytes', 'pending_bytes',
                  'pools', 'pools_bytes', 'event_handles'):
        assert final[field] == initial[field], (field, initial, final)
    for field in ('resident_bytes', 'reserved_bytes', 'pinned_bytes'):
        assert final_weights[field] == initial_weights[field]
    return Report(True, json.dumps({'passed': True, 'provenance': provenance, 'cases': cases,
                                   'initial': initial, 'final': final,
                                   'initial_weights': initial_weights, 'final_weights': final_weights,
                                   'request_owned_keys': sorted(request.working_phase_keys),
                                   'scope': 'instrumented actual producer/Loop allocation and mean-commit proof'}, sort_keys=True))
